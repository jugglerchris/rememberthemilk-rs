# Changelog

<!--
Possible log types:

- `[added]` for new features.
- `[changed]` for changes in existing functionality.
- `[deprecated]` for once-stable features removed in upcoming releases.
- `[removed]` for deprecated features removed in this release.
- `[fixed]` for any bug fixes.
- `[security]` to invite users to upgrade in case of vulnerabilities.
-->

### 0.4.11

- [added] Local task cache in an sqlite database.  This makes the TUI mode
  much more responsive, and makes it reasonable to ensure that child tasks are
  included in the UI without many slow requests.
  - sync command in the CLI to update the cache
  - the tui uses the cache, with ctrl-R to sync.
  - a subset of RTM filter syntax is supported by the cache.

### 0.4.10

- [changed] Update ratatui to 0.30 and rui-tree-widget to 0.24.
- [fixed] Fix the CI, and some format/clippy fixes.

### 0.4.9

- [changed] Updated reqwest to 0.13 and bumped other dependencies.

### 0.4.8

- [changed] Updated some more dependencies.

### 0.4.7

- [fixed] Remove unused `atty` dependency

### 0.4.6

- [added] ?/h in TUI to show keys.
- [added] ^L in TUI to refresh screen
- [added] API and TUI key (C) to mark complete
- [added] Limited undo support ("mark complete" only)
- [added] Optional console-subscriber support
- [changed] Updated some dependencies.

### 0.4.5

- [changed] Updated dependencies, including structopt -> clap.

### 0.4.4

- [changed] Updated ratatui and related crates.
