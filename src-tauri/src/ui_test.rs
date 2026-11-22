//! Checks over the pages' CSS for a cost that comes back silently: while any CSS animation runs, WebView2 draws the
//! page at the monitor's refresh rate (360 times a second here), even a stepped animation on an 8-pixel dot. v0.4.0's
//! pulsing recording dot took ~21% of the GPU that way, all day, in the tray behind a game. And the corner pop-up is
//! drawn over the game itself.

const INDEX: &str = include_str!("../../ui/index.html");
const TOAST: &str = include_str!("../../ui/toast.html");
const APP: &str = include_str!("../../ui/app.js");

/// The only rules allowed an animation that repeats forever: each is on an element that's only there for a moment.
const FOREVER_OK: [(&str, &str); 4] = [
    (".spin", "loading spinners: the first library scan, a button waiting for its answer, the Recycle Bin check"),
    (".saved.busy svg", "the editor's \"Saving edits…\" icon, from an edit until the save answers (under a second)"),
    (".rec-dot.starting", "the recording dot while the replay buffer starts, a second or two"),
    (".lane-a.loading::after", "an editor waveform lane's shimmer until that waveform is decoded"),
];

/// Every declaration in a page's `<style>` blocks, as (selector, property, value), without comments.
fn declarations(html: &str) -> Vec<(String, String, String)> {
    let mut css = String::new();
    let mut rest = html;
    while let Some(i) = rest.find("<style") {
        let block = &rest[i..];
        let open = block.find('>').expect("<style>") + 1;
        let close = block.find("</style>").expect("</style>");
        css.push_str(&block[open..close]);
        css.push('\n');
        rest = &block[close..];
    }
    let mut out = Vec::new();
    let mut blocks: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut quote = None;
    let mut chars = css.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            buf.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            '"' | '\'' => {
                quote = Some(c);
                buf.push(c);
            }
            '{' => {
                blocks.push(buf.split_whitespace().collect::<Vec<_>>().join(" "));
                buf.clear();
            }
            ';' | '}' => {
                if let (Some(sel), Some((p, v))) = (blocks.last(), buf.split_once(':')) {
                    out.push((sel.clone(), p.trim().to_lowercase(), v.trim().to_string()));
                }
                buf.clear();
                if c == '}' {
                    blocks.pop();
                }
            }
            _ => buf.push(c),
        }
    }
    out
}

/// The selectors in a selector list.
fn selectors(list: &str) -> Vec<String> {
    list.split(',').map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

/// The parts of an animation or transition value (one per comma, not counting commas inside brackets), each split
/// into words with bracketed arguments dropped: "in .28s cubic-bezier(.2, .8, .2, 1) forwards" → in .28s cubic-bezier forwards.
fn parts(value: &str) -> Vec<Vec<String>> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0);
    for c in value.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
            _ if depth == 0 => cur.push(c),
            _ => {}
        }
    }
    out.push(cur);
    out.iter().map(|p| p.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>()).filter(|w| !w.is_empty()).collect()
}

/// app.js without its comments (roughly: a `//` after a space or at the start of a line starts one, so URLs stay).
fn js_code(js: &str) -> String {
    let mut out = String::new();
    let mut rest = js;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = rest[i + 2..].find("*/").map_or("", |j| &rest[i + 2 + j + 2..]);
    }
    out.push_str(rest);
    out.lines()
        .map(|l| match l.find("// ").filter(|&i| i == 0 || l[..i].ends_with(char::is_whitespace)) {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// "0.3s" or "300ms" in seconds.
fn secs(word: &str) -> Option<f64> {
    match word.strip_suffix("ms") {
        Some(ms) => ms.parse::<f64>().ok().map(|v| v / 1000.0),
        None => word.strip_suffix('s')?.parse().ok(),
    }
}

/// No animation repeats forever: in the main window only on the short-lived states in FOREVER_OK, in the pop-up never.
#[test]
fn no_animation_runs_forever() {
    let mut wrong = Vec::new();
    let mut used = Vec::new();
    for (page, html) in [("index.html", INDEX), ("toast.html", TOAST)] {
        for (sel, prop, value) in declarations(html) {
            if !prop.ends_with("animation") && !prop.ends_with("animation-iteration-count") {
                continue;
            }
            if !value.to_lowercase().contains("infinite") {
                continue;
            }
            for s in selectors(&sel) {
                match FOREVER_OK.iter().find(|(ok, _)| *ok == s) {
                    Some((ok, _)) if page == "index.html" => used.push(*ok),
                    _ => wrong.push(format!("{page}: {s} {{{prop}: {value}}}")),
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "an animation that repeats forever makes the page draw at the monitor's refresh rate while it shows; leave it \
         static, or add a short-lived state to FOREVER_OK:\n{}",
        wrong.join("\n")
    );
    // Entries no longer used come out, so the list can't quietly let one back in.
    for (ok, why) in FOREVER_OK {
        assert!(used.contains(&ok), "FOREVER_OK lists {ok} ({why}), which no longer animates forever");
    }
    // Nor from JS (an inline style, or the Web Animations API).
    let code = js_code(APP).replace(' ', "");
    assert!(!code.contains("infinite") && !code.contains("iterations:Infinity"), "app.js starts an animation that repeats forever");
}

/// The recording dot is static. It shows the whole time the buffer runs, so any animation or transition on it keeps the
/// page drawing: a smooth pulse at the monitor's refresh rate took a fifth of the GPU (v0.4.0), and even a stepped one
/// or a 10 Hz timer cost ~4.6% of a CPU core while focused, against 0.3% static (measured on the 360 Hz screen).
#[test]
fn recording_dot_has_no_css_animation_or_transition() {
    let mut wrong = Vec::new();
    for (sel, prop, value) in declarations(INDEX) {
        let moving = (prop.contains("animation") && value != "none") || prop.contains("transition");
        // The starting dot is a spinner instead (see FOREVER_OK).
        let dots = selectors(&sel).into_iter().filter(|s| s.contains("rec-dot") && s != ".rec-dot.starting").count();
        if moving && dots > 0 {
            wrong.push(format!("{sel} {{{prop}: {value}}}"));
        }
    }
    assert!(wrong.is_empty(), "the recording dot stays static:\n{}", wrong.join("\n"));
}

/// The "clip saved" pop-up is drawn over the game: it only animates while it slides in and out, a fraction of a second
/// each (v0.4.1's 3 s draining bar had it drawn at the monitor's refresh rate for its whole life).
#[test]
fn toast_only_animates_briefly() {
    let mut checked = 0;
    for (sel, prop, value) in declarations(TOAST) {
        let animation = prop.ends_with("animation") || prop.ends_with("animation-duration");
        if !animation && !prop.ends_with("transition") && !prop.ends_with("transition-duration") {
            continue;
        }
        for words in parts(&value) {
            if words == ["none"] {
                continue;
            }
            let duration = words.iter().find_map(|w| secs(w)).unwrap_or(0.0);
            assert!(duration <= 0.5, "toast.html: {sel} {{{prop}: {value}}} runs {duration} s over the game");
            if animation {
                let repeats: Vec<&String> = words.iter().filter(|w| *w == "infinite" || w.parse::<f64>().is_ok_and(|n| n != 1.0)).collect();
                assert!(repeats.is_empty(), "toast.html: {sel} {{{prop}: {value}}} repeats");
            }
            checked += 1;
        }
    }
    assert!(checked >= 2, "the slide in and out were not found ({checked})");
}

/// The pop-up window is made once and reused: rec_app::toast sets window.__TOAST for the first one and calls show()
/// for each one after. Without them, a reused pop-up would show old text.
#[test]
fn toast_page_keeps_what_the_reused_window_calls() {
    assert!(TOAST.contains("window.__TOAST") && TOAST.contains("function show("), "toast.html lost window.__TOAST or show()");
    let rec_app = include_str!("rec_app.rs");
    assert!(rec_app.contains("window.__TOAST = {data};") && rec_app.contains("show({data})"), "rec_app::toast no longer uses them");
}

#[test]
fn css_parsing() {
    let css = "<style>/* a{animation:x 1s infinite} */ .a, .b .c{color:red;animation: spin .9s linear infinite}\
               @media (max-width:900px){.d::after{content:\"{;}\";transition:opacity .2s}}</style>";
    let d = declarations(css);
    assert_eq!(d.len(), 4, "{d:?}");
    assert_eq!(d[1], (".a, .b .c".into(), "animation".into(), "spin .9s linear infinite".into()));
    assert_eq!(d[3], (".d::after".into(), "transition".into(), "opacity .2s".into()));
    assert_eq!(selectors(".a, .b  .c"), vec![".a", ".b .c"]);
    assert_eq!(parts("in .28s cubic-bezier(.2, .8, .2, 1) forwards, out .3s ease-in 2.9s"), vec![
        vec!["in", ".28s", "cubic-bezier", "forwards"],
        vec!["out", ".3s", "ease-in", "2.9s"],
    ]);
    assert_eq!((secs(".28s"), secs("300ms"), secs("forwards")), (Some(0.28), Some(0.3), None));
}
