# Performance log

Measured on the installed game (Season 10 build): 12th-gen i7-12700K, Linux,
release build. Each number is a range over 3 runs. Read this before trying
another optimization, so ideas that didn't pay off aren't re-run.

The budgets are guarded by `cargo test --release -- --ignored --exact swap::perf_budget`
(database build under 5 s, whole swap job under 9 s).

## 2026-10-19

Baseline: opening the game files 1.3 s, skin index 0.47 s, database build
14.0–14.4 s, skin swap (`create`) 5.7–5.9 s, all table mods 0.85 s. A swap
in the app (open, create, open again, tables) took about 9 s.

| Idea | Baseline → Result | Verdict | Why |
|---|---|---|---|
| Pre-filter animation overrides on zen header names (`Game::header_names`) instead of fully extracting all ~825 default-skin packages | override scan 3.5–3.9 s → 0.56–0.62 s; `create` 5.7–5.9 s → 2.7–2.9 s | kept | Full legacy extraction just to read names was most of a swap. Same packages found (11 / 29); swap tests green. |
| Reuse the swap's open game for the table update (no second `Game::open` and skin index) | swap job 6.11–6.19 s → 4.68–4.74 s | kept | Saved one open (1.3 s) and one skin index (0.47 s) per swap. |
| Read variant-cue names from zen headers in the database build | variant check 11.5–12.9 s → 8–12 ms; build 14.0–14.4 s → 2.34–2.40 s | kept | Particle systems are imported packages, which the header lists. Same 56 skins flagged. |

Where the time goes now (database build, about 2.4 s): reading the localization
0.6 s, skin index 0.6 s, sound checks about 1 s (they still extract each skin's
sound table). A swap job (about 4.7 s): opening the game 1.3 s, mesh, cue and
override work about 2.6 s, table mods 0.85 s (UAssetTool packing).
