/* =====================================================================
   What's new: Backtrack's release notes, shipped inside the app (no network, works offline).
   One entry per release, newest first. Written for people (not commits) as part of making each release,
   and kept for every release so a catch-up across several versions, and browsing old ones, both work.

   release  { version, date: 'YYYY-MM-DD', title: a few words that sum it up, items: [item, …] }
   item     { kind:  'new' | 'improved' | 'fixed'
              text:  the change itself, one short sentence-case line
              more:  optional second line, said plainly
              keys:  optional key chips, e.g. ['Ctrl', 'K']
              go:    optional link into the app: a route ('#settings', '#settings-encoder') or an action: 'find' (Find a setting),
                     'buffer-menu' (the replay buffer menu) or 'restart' (that menu, on Restart Backtrack)
              goLabel: optional button text for the link (default 'Show me')
              highlight: true when it's worth telling people about by itself. A release with no highlight is a quiet
                     one: the version number changes and its notes are there to read, but nothing asks for attention,
                     and its notes wait to be shown with the next release that has one. }
   ===================================================================== */
window.BT_NOTES = [
  { version: '0.4.8', date: '2026-11-25', title: 'Thumbnails follow your trim', items: [
    { kind: 'improved', text: 'A trimmed clip’s thumbnail shows where it starts', more: 'Move the In point and the thumbnail becomes the first frame of your trim, the one your export starts on. Clips you’ve already trimmed get theirs too.', highlight: true },
  ] },
  { version: '0.4.7', date: '2026-11-24', title: 'Updates tell you what’s new', items: [
    { kind: 'new', text: 'See what’s new after an update', more: 'The version in the title bar says when Backtrack has updated. Click it for what changed.', highlight: true },
    { kind: 'improved', text: 'Updates wait until you’re out of your game', more: 'Backtrack won’t restart to install one while a game is in front, so the replay buffer never empties mid-match.', highlight: true },
    { kind: 'improved', text: 'The “updated” pop-up only shows on your desktop', more: 'Never over a game, and with its own icon so it can’t pass for “Clip saved”.' },
  ] },
  { version: '0.4.6', date: '2026-11-24', title: 'A tidier Settings', items: [
    { kind: 'new', text: 'Find a setting', keys: ['Ctrl', 'K'], more: 'Type what you want, like “make clips smaller”, and jump straight to it.', go: 'find', goLabel: 'Try it', highlight: true },
    { kind: 'improved', text: 'A new layout for Settings', more: 'Six short pages with a sidebar, instead of one long scroll.', go: '#settings', goLabel: 'Open Settings', highlight: true },
    { kind: 'new', text: 'Problems show in the Settings sidebar', more: 'A recorder error, a hotkey clash, missing FFmpeg or a missing folder gets a badge.', go: '#settings', goLabel: 'Open Settings' },
    { kind: 'improved', text: 'Pick the encoder from one dropdown', go: '#settings-encoder' },
    { kind: 'improved', text: 'The replay buffer menu works with the keyboard', go: 'buffer-menu' },
  ] },
  { version: '0.4.5', date: '2026-11-24', title: 'Recovers from driver crashes', items: [
    { kind: 'improved', text: 'Recovers from graphics driver crashes', more: 'If your driver crashes or resets, the replay buffer restarts by itself and a pop-up tells you.', highlight: true },
    { kind: 'new', text: 'Restart Backtrack', more: 'From the tray menu, or from the replay buffer menu when something goes wrong.', go: 'restart' },
  ] },
  { version: '0.4.4', date: '2026-11-22', title: 'Version in the title bar', items: [
    { kind: 'new', text: 'The version number shows in the title bar' },
  ] },
  { version: '0.4.3', date: '2026-11-22', title: 'No more stutter in long sessions', items: [
    { kind: 'fixed', text: 'Clips no longer stutter after about an hour of recording', more: 'Sound and picture slowly drifted apart.', highlight: true },
  ] },
  { version: '0.4.2', date: '2026-11-22', title: 'Lighter in the background', items: [
    { kind: 'improved', text: 'Backtrack does less work in the tray and while you play' },
  ] },
  { version: '0.4.1', date: '2026-11-22', title: 'Smoother clips', items: [
    { kind: 'improved', text: 'Smoother clips from busy games and games with uncapped frame rates', highlight: true },
    { kind: 'improved', text: 'The window stops drawing when it’s hidden', more: 'Less work for your graphics card.' },
  ] },
  { version: '0.4.0', date: '2026-11-22', title: 'Updates itself', items: [
    { kind: 'new', text: 'Backtrack updates itself', more: 'It installs a new version when it starts. You can turn this off.', go: '#settings-updates', highlight: true },
    { kind: 'new', text: 'Drag an export straight into Discord', more: 'From the screen you see when an export is done.', highlight: true },
    { kind: 'improved', text: 'Small improvements to the editor' },
  ] },
];
