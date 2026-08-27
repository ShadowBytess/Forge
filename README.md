# Forge

**A graphical package manager for Arch Linux — pacman, AUR and Flatpak behind
one clean, modern interface.**

Forge is a GTK4 / libadwaita frontend (with a matching CLI) that orchestrates
existing package-management systems. It deliberately does **not** reimplement
pacman's dependency resolution: pacman remains the single source of truth for
repository packages, its `--print-format` output defines every transaction
preview, and removals are computed by pacman itself (`-Rs` semantics).

```
┌────────────────────────────────────────────────────────────┐
│  GUI (GTK4 + libadwaita)          CLI (clap)               │
│      └───────────┬───────────────────┘                    │
│            transaction engine                           │
│   preview → confirm → execute → structured events       │
│      ┌───────────┼───────────────────┐                    │
│  pacman backend   AUR backend      Flatpak backend        │
│  (argv+polkit)   (RPC v5+makepkg) (--columns)           │
│      │                │                     │            │
│    pacman           git/makepkg          flatpak         │
└────────────────────────────────────────────────────────────┘
```

## Features

- **Unified search** across official repositories, the AUR and Flatpak with
  installed-status filtering.
- **Rich package details**: name, version, source, description,
  architecture, download/installed sizes, dependencies, optional
  dependencies, conflicts, homepage, licenses, maintainer, AUR votes and
  out-of-date flags; file lists for installed repository packages.
- **Installed page** with filtering, sorting (name/version/size/source),
  reinstall and remove.
- **Updates page** showing current → new versions and download sizes, with
  selective or full updates across all backends.
- **Transaction system** representing install / remove / upgrade /
  reinstall: a full *preview* (targets, resolver-added dependencies,
  no-longer-needed removals, conflicts, sizes, warnings) is always shown;
  execution streams structured progress events (current package, operation,
  download progress, tool output, errors, final result) into the UI and can
  be **cancelled** where the underlying tool supports it safely.
- **Dedicated AUR workflow**: PKGBUILD review dialog before any build,
  build dependencies surfaced from `.SRCINFO`, unprivileged `makepkg`
  builds, polkit-guarded installation of the resulting archive.
- **Flatpak scope awareness**: per-user vs system-wide is displayed and
  respected; user-scope operations never ask for authentication.

## Security model

Forge treats package management as a security-sensitive activity:

| Rule | How it is enforced |
| --- | --- |
| No shell, ever | All external tools run via `tokio::process` argv vectors; there is no code path that builds a shell string. |
| Input validation | Every user-supplied identifier passes `validate::ensure_valid_identifier` (charset, length, leading `-` rejection) before reaching argv. |
| No sudo | Privileged operations spawn `pkexec <tool> <args>` so the standard polkit agent performs authentication. Dismissed prompts map to a clean "authentication denied" state. |
| Least privilege | Read-only queries, AUR builds and user-scope Flatpak operations never escalate at all. |
| Informed AUR installs | AUR packages are labelled community-produced everywhere; building requires an explicit review step (configurable, on by default). |
| Native tools decide | Dependency closures, conflict detection and removal cascades come from pacman/flatpak output that is machine-stable (`--print-format`, `--columns`) — never from prose parsing or Forge's own resolver. |

## Backends

All backends implement one async trait (`forge::backend::Backend`), which is
all the UI ever sees:

```rust
#[async_trait]
pub trait Backend: Send + Sync {
    fn id(&self) -> BackendId;
    fn capabilities(&self) -> Capabilities;
    async fn available(&self) -> bool;
    async fn search(&self, query: &str) -> Result<Vec<Package>>;
    async fn info(&self, id: &PackageId) -> Result<Option<Package>>;
    async fn installed(&self) -> Result<Vec<Package>>;
    async fn updates(&self) -> Result<Vec<UpdateEntry>>;
    async fn preview(&self, actions: &[Action]) -> Result<TransactionPreview>;
    async fn execute(&self, actions, events, cancel) -> Result<()>;
}
```

Adding another package format (Nix, AppImage catalogs, …) means implementing
this trait and registering it in `Registry::with_config` — no UI changes.
A deterministic `MockBackend` ships in-tree for tests and UI development.

## Building

Requirements (Arch package names):

- `rust` ≥ 1.85 (edition 2024)
- `gtk4` ≥ 4.14, `libadwaita` ≥ 1.5 (development packages:
  `gtk4-devel` / `libadwaita-devel` equivalents; on Arch install `gtk4`
  and `libadwaita`)
- `pacman` (obviously), optionally `flatpak` and `base-devel` for AUR builds

```sh
git clone <repo> forge && cd forge
cargo build --release
```

The GUI is a default feature; build a headless/CLI-only binary with:

```sh
cargo build --release --no-default-features
```

### Install

```sh
sudo make install          # binary, desktop file, icon, man page, example config
```

or just the binary via `cargo install --path .`.

## CLI usage

```sh
forge                          # launch the GUI
forge search firefox           # search everything enabled
forge search -s aur yay        # search only the AUR
forge info firefox             # detailed metadata
forge install firefox          # preview → confirm → install
forge install --reviewed yay   # AUR install after PKGBUILD review
forge remove firefox           # preview shows pacman's removal plan
forge update                   # refresh dbs + upgrade all backends
forge list --source flatpak    # list installed per backend
forge --help
```

The CLI prints the same transaction preview the GUI shows and asks for
confirmation on terminals (`--yes` to skip). Non-interactive runs without
`--yes` abort rather than guess.

## Configuration

`~/.config/forge/config.toml` — see
[`docs/examples/forge-config.toml`](docs/examples/forge-config.toml) for a
fully commented example. Highlights: `general.confirm_before_transaction`,
per-source enable switches, `sources.preferred_repositories`,
`aur.require_pkgbuild_review` (security default: true), `aur.build_directory`,
Flatpak scope visibility, window/colour-scheme preferences and parallel
download limits.

## Project layout

```
src/
├── main.rs             entry point (GUI owns the main thread)
├── cli.rs              clap-based CLI sharing the backends
├── error.rs            unified error type
├── util/               cancel token, safe process runner, validation, sizes
├── config/             ~/.config/forge/config.toml (serde + defaults)
├── cache/              TTL memory cache for slow sources
├── package/            shared package/update/filter model
├── backend/
│   ├── mod.rs          Backend trait, Capabilities, Registry
│   ├── pacman/         safe argv + machine-stable parsers
│   ├── aur/            RPC v5 client, vercmp, SRCINFO, guarded build flow
│   ├── flatpak/        --columns TSV parsers, scope handling
│   └── mock.rs         deterministic test backend
├── transaction/        Action, TransactionPreview, event stream, executor
├── authentication/     pkexec/polkit integration (no sudo)
├── app/                GUI bootstrap
└── ui/                 libadwaita pages, widgets, tokio↔GTK bridge
```

## Testing

```sh
cargo test                # 60+ unit tests
cargo clippy --all-targets
cargo fmt --check
```

Covered areas include pacman version comparison (vercmp edge cases), output
parsers fed with real-format fixtures, identifier validation against
injection attempts, size parsing/formatting, configuration round-trips, the
cache, pkexec argument construction, the AUR review gate and the transaction
executor's event ordering/cancellation using the mock backend.

## Roadmap ideas

- Flatpak runtime/SDK awareness and `flatpak-search` appstream metadata
- Download-progress percentages from pacman via external fetch
- Transaction history / undo hints
- Per-repository pinning and ignore rules

## License

MIT — see [LICENSE](LICENSE).
