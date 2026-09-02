# Repository Guidelines

## Project Structure & Module Organization

FlowLens is a Windows network-traffic monitor built with Tauri 2, Svelte 5, and Rust. The Svelte frontend is in `src/`: `App.svelte` is the dashboard, `Floating.svelte` and `Settings.svelte` serve secondary windows, and reusable charts and tables live in `src/lib/components/`. Shared frontend state, Tauri bindings, and visual tokens belong in `src/lib/`.

Rust backend code is in `src-tauri/src/`. Keep Tauri commands and application wiring in `lib.rs`; place capture, port attribution, adapter I/O, policy, and installed-software logic in their focused modules. SQLite history types and storage stay under `src-tauri/src/traffic_history/`. Static application icons belong in `src-tauri/icons/`; screenshots and design documentation belong in `docs/`.

## Build, Test, and Development Commands

- `npm install` installs the pinned frontend toolchain.
- `npm run dev` starts the Vite frontend only.
- `npm run tauri dev` launches the full desktop app for development.
- `npm run build` creates the production web bundle.
- `npm run tauri build` creates the Windows executable and NSIS bundle.
- `cargo test --manifest-path src-tauri/Cargo.toml` runs Rust unit tests.
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check` verifies Rust formatting.

Use Node.js 18+ and stable Rust. Packet capture requires Npcap installed in WinPcap API-compatible mode; the app can otherwise start but cannot provide process-level capture data.

## Coding Style & Naming Conventions

Follow the existing two-space indentation in `.svelte`, `.ts`, and CSS files. Use TypeScript types for Tauri payloads and Svelte 5 runes (`$state`, `$derived`, `$props`) for reactive state. Name Svelte components in PascalCase (for example, `AppTrafficTable.svelte`); use camelCase for TypeScript identifiers.

Use `rustfmt`-formatted Rust, `snake_case` modules/functions, and `PascalCase` types. Keep capture-loop work allocation-conscious: do not add avoidable per-packet allocations or blocking I/O. Preserve the existing command/event contracts between `src/lib/tauri.ts` and Rust.

## Testing Guidelines

Add Rust unit tests beside the behavior they cover using `#[cfg(test)] mod tests`; name them `test_<behavior>`. In-memory SQLite tests are appropriate for history-store changes. There is no configured frontend test runner, so manually validate affected views with `npm run tauri dev`, including IPv4/IPv6 filters, adapter selection, and empty/loading states.

## Commit & Pull Request Guidelines

Recent history uses concise Chinese summaries, sometimes with Conventional Commit prefixes such as `fix:`. Use an imperative, scoped subject, for example `fix: avoid duplicate adapter rows`. Keep unrelated changes separate. Pull requests should explain user-visible behavior, list verification commands, link relevant issues, and include screenshots for UI changes. Flag Windows, Npcap, database-schema, or data-retention implications explicitly.
