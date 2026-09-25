# Changelog

## [0.1.17](https://github.com/planetaryescape/ms-todo/compare/v0.1.16...v0.1.17) (2026-09-25)


### Features

* move tasks between lists without losing anything (rung 5e) ([592c1d9](https://github.com/planetaryescape/ms-todo/commit/592c1d92c2e0a830bf11222471dcbbf5bb683a5b))


### Bug Fixes

* compare timestamps only where they are timestamps, and recheck the source right before delete ([c4e68ba](https://github.com/planetaryescape/ms-todo/commit/c4e68ba1be61dba6c9cf01a10f06b3c42d1648e8))
* verify every surviving field and recheck before deleting after a restart ([e912600](https://github.com/planetaryescape/ms-todo/commit/e912600644e26001cf8b6d912f148819faa1e9f4))


### Documentation

* rung 5e in the README, blueprint, roadmap, S14, D-051 and the skill ([e1bba22](https://github.com/planetaryescape/ms-todo/commit/e1bba224d19b96a97b0c7c22ff54f31250d98539))

## [0.1.16](https://github.com/planetaryescape/ms-todo/compare/v0.1.15...v0.1.16) (2026-09-25)


### Features

* add one-dark, kanagawa, night-owl and cobalt2 themes ([8093473](https://github.com/planetaryescape/ms-todo/commit/80934737d7978f4342373165d727a697f6c9d02c))

## [0.1.15](https://github.com/planetaryescape/ms-todo/compare/v0.1.14...v0.1.15) (2026-09-25)


### Features

* follow links from a task ([eb02bb2](https://github.com/planetaryescape/ms-todo/commit/eb02bb21b105aadf32b9e95f48fdb144133c5fbf))
* themes for the TUI ([e6cb334](https://github.com/planetaryescape/ms-todo/commit/e6cb3341223dfe4997b9e2cfdeeb73885e43ec6e))


### Bug Fixes

* never print control characters in ids output ([0dad23a](https://github.com/planetaryescape/ms-todo/commit/0dad23aae8869a65ed0b770fc83cc6f639a644bf))


### Documentation

* links in the README, blueprint, skill and D-050 ([eeadeb7](https://github.com/planetaryescape/ms-todo/commit/eeadeb73e6d633bc080be5b6f7dbbaef0d153d60))
* themes in the README, blueprint and D-049 ([fe7e142](https://github.com/planetaryescape/ms-todo/commit/fe7e142ed993e4906892c818a7495c3c2c5bf4b1))

## [0.1.14](https://github.com/planetaryescape/ms-todo/compare/v0.1.13...v0.1.14) (2026-09-25)


### Features

* see what I finished and clear what's overdue (rung 5d) ([a7b6361](https://github.com/planetaryescape/ms-todo/commit/a7b6361631e6df9a1b126eb10828f2a1159632c1))


### Bug Fixes

* read completion dates as the UTC date Graph records ([c758c5a](https://github.com/planetaryescape/ms-todo/commit/c758c5aa7bbca7d2726dc09f504c1f2851c55a2a))


### Documentation

* rung 5d in the README, blueprint, roadmap, D-048 and the skill ([c8b31dc](https://github.com/planetaryescape/ms-todo/commit/c8b31dc93d42488b86e0063763d137455d29725c))

## [0.1.13](https://github.com/planetaryescape/ms-todo/compare/v0.1.12...v0.1.13) (2026-09-25)


### Features

* group lists into folders (rung 5c) ([4ff31af](https://github.com/planetaryescape/ms-todo/commit/4ff31af8c1522772382bb149bf4345d60068de94))


### Bug Fixes

* undo refuses when the field changed since ([1d0c645](https://github.com/planetaryescape/ms-todo/commit/1d0c6451e4b1b53434ccedca27451c9bbba3d1f9))


### Documentation

* folders in the README, blueprint, roadmap rung 5c, D-047 and the skill ([2597059](https://github.com/planetaryescape/ms-todo/commit/2597059c30ee459f1b2ce81e52742742ad00f73e))

## [0.1.12](https://github.com/planetaryescape/ms-todo/compare/v0.1.11...v0.1.12) (2026-09-25)


### Bug Fixes

* a reminder with only a day defaults to 09:00 ([4c4b25d](https://github.com/planetaryescape/ms-todo/commit/4c4b25d8157fe16682629d12f506de7e01b444f1))

## [0.1.11](https://github.com/planetaryescape/ms-todo/compare/v0.1.10...v0.1.11) (2026-09-25)


### Bug Fixes

* fully detach the auto-started daemon and treat a zombie as exited ([6a271ea](https://github.com/planetaryescape/ms-todo/commit/6a271eab00ff55261de90da10487318aa1aab9d2))

## [0.1.10](https://github.com/planetaryescape/ms-todo/compare/v0.1.9...v0.1.10) (2026-09-25)


### Features

* --due, --reminder and --importance take phrases ([471bf58](https://github.com/planetaryescape/ms-todo/commit/471bf58f6959f3f7b1d1a48182d3277de210c054))
* read date phrases and importance levels in a new nlp crate ([56753eb](https://github.com/planetaryescape/ms-todo/commit/56753eb371abb2c62a6704e4fbe7809ffa4dc782))


### Bug Fixes

* edit any task field with a real line editor and a field picker ([6b2185f](https://github.com/planetaryescape/ms-todo/commit/6b2185fa305d62430766036e33cfe5ae7b4777ad))


### Documentation

* line editor, field picker, date phrases and D-045 ([36af625](https://github.com/planetaryescape/ms-todo/commit/36af625fe6d193a55345e800f1e8c1fa18fb8180))

## [0.1.9](https://github.com/planetaryescape/ms-todo/compare/v0.1.8...v0.1.9) (2026-09-24)


### Features

* edit fields, act on a selection, run the palette and see diagnostics in the TUI ([443e522](https://github.com/planetaryescape/ms-todo/commit/443e522c11f4fba11fea3ca56a3bc16e6462e73f))
* open the TUI when ms-todo or mst runs with no command in a terminal ([3292001](https://github.com/planetaryescape/ms-todo/commit/3292001a721dafeb9f4892406266c2ed6f7412ab))


### Bug Fixes

* never miss a task cached mid-sync, and never hang connecting to a stopping daemon ([6263706](https://github.com/planetaryescape/ms-todo/commit/6263706be20067bf9622578786d1475aacc3c35a))
* strip control characters from terminal output ([69251d0](https://github.com/planetaryescape/ms-todo/commit/69251d0329a64753b1c9565985e56c69718a5212))


### Documentation

* rung 5b status, keys, Homebrew install, roadmap and D-044 ([e48f3ae](https://github.com/planetaryescape/ms-todo/commit/e48f3aea760854985113db7afb93ee6cc9f08e79))

## [0.1.8](https://github.com/planetaryescape/ms-todo/compare/v0.1.7...v0.1.8) (2026-09-24)


### Features

* browse and act on your tasks in mst tui ([9d57a1a](https://github.com/planetaryescape/ms-todo/commit/9d57a1a193780d6deeeae795e9c83fda1eae8959))


### Bug Fixes

* never act on a stale list after switching scope ([135151b](https://github.com/planetaryescape/ms-todo/commit/135151b06f039e94bccf96f3ed0a066a7605165e))


### Documentation

* add rungs 5a and 5b, D-043, and the TUI to the README ([9882c5e](https://github.com/planetaryescape/ms-todo/commit/9882c5e35c1faa939a33e7d5c41f3ca4a0ef6bdb))

## [0.1.7](https://github.com/planetaryescape/ms-todo/compare/v0.1.6...v0.1.7) (2026-09-24)


### Features

* find any task by the words in it with search ([69fa990](https://github.com/planetaryescape/ms-todo/commit/69fa9907e26bf870bd4ed3b4197e3c73ec0765a7))


### Documentation

* add rung 4b search, D-041 and D-042 ([59e71eb](https://github.com/planetaryescape/ms-todo/commit/59e71ebdedf3f86d112e51a6713728407e1ab66c))

## [0.1.6](https://github.com/planetaryescape/ms-todo/compare/v0.1.5...v0.1.6) (2026-09-24)


### Features

* queue writes offline in an outbox, with undo ([4765995](https://github.com/planetaryescape/ms-todo/commit/4765995859a67beff60e7f2b62d54fbbb56a90e7))


### Bug Fixes

* close outbox races and report a too-new database on linux ([5429309](https://github.com/planetaryescape/ms-todo/commit/5429309f4e2d770ca8af5064ec06ba07bdd3dc8d))
* retry refuses an op whose dependency did not succeed ([ba9a267](https://github.com/planetaryescape/ms-todo/commit/ba9a267a5937da27b2a95afb6711c860821d6f2c))

## [0.1.5](https://github.com/planetaryescape/ms-todo/compare/v0.1.4...v0.1.5) (2026-09-24)


### Features

* add mst as an official alias ([f0b305c](https://github.com/planetaryescape/ms-todo/commit/f0b305cc59762009bfa1b5700d20986859a9508d))

## [0.1.4](https://github.com/planetaryescape/ms-todo/compare/v0.1.3...v0.1.4) (2026-09-24)


### Features

* keep the cache live with delta sync ([cea04e4](https://github.com/planetaryescape/ms-todo/commit/cea04e4a67e9896b1c0f29d090537ec5933baaa4))


### Documentation

* record phone check results and D-037 ([74cf4d7](https://github.com/planetaryescape/ms-todo/commit/74cf4d7720442f57917984f0123763fc8a6268f9))

## [0.1.3](https://github.com/planetaryescape/ms-todo/compare/v0.1.2...v0.1.3) (2026-09-24)


### Features

* answer reads instantly from a local cache kept in sync with Graph ([2a9fc5f](https://github.com/planetaryescape/ms-todo/commit/2a9fc5fbfb624e8d0bbca169dbbdd8656d6e0859))


### Bug Fixes

* private cache files and keep idempotency keys on uncertain writes ([a2b09ff](https://github.com/planetaryescape/ms-todo/commit/a2b09ff8c244f2c3ec0a029002035a72c76a0dba))

## [0.1.2](https://github.com/planetaryescape/ms-todo/compare/v0.1.1...v0.1.2) (2026-09-24)


### Features

* capture and finish tasks from the CLI ([0d67c46](https://github.com/planetaryescape/ms-todo/commit/0d67c460caf85e8f5055f4a32ceaf89f3cfa68e9))


### Bug Fixes

* report outcome_unknown when a sent mutation loses its reply ([557d161](https://github.com/planetaryescape/ms-todo/commit/557d1611ab7d8cea3ac12ad3144cb82cdcb91de9))


### Documentation

* move schema command to rung 3a ([d4e3738](https://github.com/planetaryescape/ms-todo/commit/d4e3738ce9e094dfa0e89bcd8273bf9ffbd321b1))

## [0.1.1](https://github.com/planetaryescape/ms-todo/compare/v0.1.0...v0.1.1) (2026-09-24)


### Features

* see lists and tasks through a minimal daemon ([5ff769a](https://github.com/planetaryescape/ms-todo/commit/5ff769aa751941d44345db7ee64c440e0d5e1fc5))


### Bug Fixes

* add schema_version to jsonl records ([f41ec30](https://github.com/planetaryescape/ms-todo/commit/f41ec30cf4fd385281233b95d47b952e1a530ae9))


### Documentation

* document rung 1 and config in ~/.config ([dc714dd](https://github.com/planetaryescape/ms-todo/commit/dc714dd7a14a50625fa3d23bdc21818f069ee5ff))
* note auth refreshes and the IPC stall deadline ([88d4c52](https://github.com/planetaryescape/ms-todo/commit/88d4c52d2aa206c8ba530b9417f20e1770e4c25e))

## 0.1.0 (2026-09-24)


### Features

* add install.sh and install and sign-in docs ([5859f6e](https://github.com/planetaryescape/ms-todo/commit/5859f6e7310e0f217d23574d062fb38e288b9695))
* add ms-todo auth login, status and logout ([0cf5234](https://github.com/planetaryescape/ms-todo/commit/0cf5234796645d58c24203f14d323ad7fcbe30fd))
