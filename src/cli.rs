//! Command-line interface. Uses exactly the same backend abstraction,
//! registry and transaction engine as the GUI.

use std::io::{IsTerminal, Write};

use clap::{Parser, Subcommand, ValueEnum};

use crate::backend::{BackendId, Registry};
use crate::config::Config;
use crate::error::ForgeError;
use crate::package::Source;
use crate::transaction::{
    ActionKind, TransactionPreview, TxEvent, TxEventSender, execute as run_transaction,
};
use crate::util::cancel::CancelToken;
use crate::util::size::format_size;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum SourceArg {
    /// Official repositories (pacman).
    Repos,
    /// Arch User Repository.
    Aur,
    /// Flatpak remotes.
    Flatpak,
}

impl SourceArg {
    fn backend(self) -> BackendId {
        match self {
            SourceArg::Repos => BackendId::Pacman,
            SourceArg::Aur => BackendId::Aur,
            SourceArg::Flatpak => BackendId::Flatpak,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "forge",
    about = "A package manager frontend for Arch Linux (pacman, AUR, Flatpak)",
    version,
    arg_required_else_help = false
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Launches the graphical interface (main thread only).
pub fn gui_entry() -> i32 {
    let cfg = Config::load();
    crate::ui::launch(cfg)
}

#[derive(Subcommand)]
pub enum Command {
    /// Search across enabled sources.
    Search {
        query: String,
        /// Restrict to a source.
        #[arg(short, long, value_enum)]
        source: Option<SourceArg>,
        /// Only show installed packages.
        #[arg(long)]
        installed: bool,
    },
    /// Show detailed information for one package.
    Info { name: String },
    /// Install packages (shows a preview first).
    Install {
        names: Vec<String>,
        /// Skip confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
        /// AUR only: confirm the PKGBUILD was reviewed.
        #[arg(long)]
        reviewed: bool,
        /// Show the preview and exit.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove packages (shows a preview first).
    Remove {
        names: Vec<String>,
        #[arg(long, short = 'y')]
        yes: bool,
        /// Show the preview and exit.
        #[arg(long)]
        dry_run: bool,
    },
    /// Refresh databases and apply all available updates.
    Update {
        #[arg(long, short = 'y')]
        yes: bool,
        /// AUR only: confirm all AUR PKGBUILDs were reviewed.
        #[arg(long)]
        reviewed: bool,
        /// Show the preview and exit.
        #[arg(long)]
        dry_run: bool,
    },
    /// List installed packages per source.
    List {
        #[arg(short, long, value_enum)]
        source: Option<SourceArg>,
    },
    /// Print the config file path.
    ConfigPath,
}

pub async fn run(cli: Cli) -> i32 {
    let cfg = Config::load();
    let Some(command) = cli.command else {
        // No subcommand: launch the GUI (or explain how to build it).
        return crate::ui::launch(cfg);
    };
    let registry = Registry::with_config(&cfg);
    let result: Result<(), ForgeError> = match command {
        Command::Search {
            query,
            source,
            installed,
        } => search_cmd(&registry, &query, source.map(|s| s.backend()), installed).await,
        Command::Info { name } => info_cmd(&registry, &name).await,
        Command::Install {
            names,
            yes,
            reviewed,
            dry_run,
        } => install_cmd(&registry, &cfg, &names, yes, reviewed, dry_run).await,
        Command::Remove {
            names,
            yes,
            dry_run,
        } => remove_cmd(&registry, &cfg, &names, yes, dry_run).await,
        Command::Update {
            yes,
            reviewed,
            dry_run,
        } => update_cmd(&registry, &cfg, yes, reviewed, dry_run).await,
        Command::List { source } => list_cmd(&registry, source.map(|s| s.backend())).await,
        Command::ConfigPath => {
            println!("{}", crate::config::default_config_path().display());
            Ok(())
        }
    };

    match result {
        Ok(()) => 0,
        Err(ForgeError::Cancelled) => {
            eprintln!("forge: cancelled");
            130
        }
        Err(ForgeError::AuthDenied) => {
            eprintln!("forge: authentication denied");
            1
        }
        Err(e) => {
            eprintln!("forge: error: {e}");
            1
        }
    }
}

async fn search_cmd(
    registry: &Registry,
    query: &str,
    backend: Option<BackendId>,
    installed_only: bool,
) -> Result<(), ForgeError> {
    let filter = crate::package::SearchFilter {
        backends: backend.into_iter().collect(),
        status: if installed_only {
            crate::package::StatusFilter::InstalledOnly
        } else {
            crate::package::StatusFilter::Any
        },
    };
    let results = registry.search(query, &filter).await;
    let mut total = 0usize;
    for (id, res) in results {
        let Ok(packages) = res else { continue };
        if !packages.is_empty() && total > 0 {
            println!();
        }
        if !packages.is_empty() {
            println!("== {} ==", id.label());
        }
        for p in packages.iter().take(30) {
            println!(
                "{:38} {:22} {:18} {}{}",
                truncate(&p.name, 38),
                truncate(&p.version, 22),
                p.source_label(),
                mark_installed(p),
                if p.description.is_empty() {
                    String::new()
                } else {
                    format!("  {}", truncate(&p.description, 60))
                }
            );
        }
        total += packages.len();
    }
    if total == 0 {
        println!("no results for '{query}'");
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max - 1).collect::<String>() + "…"
    }
}

fn mark_installed(p: &crate::package::Package) -> &'static str {
    if p.is_installed() { "[installed]" } else { "" }
}

async fn info_cmd(registry: &Registry, name: &str) -> Result<(), ForgeError> {
    let mut found = None;
    for backend in registry.active().await {
        match backend.info(&name.to_string()).await {
            Ok(Some(pkg)) => {
                found = Some(pkg);
                break;
            }
            _ => continue,
        }
    }
    let Some(pkg) = found else {
        return Err(ForgeError::InvalidInput(format!(
            "'{name}' not found in any enabled source"
        )));
    };
    print_package_details(&pkg);
    Ok(())
}

fn print_package_details(p: &crate::package::Package) {
    println!("Name         : {}", p.name);
    println!("Version      : {}", p.version);
    println!("Source       : {} ({})", p.source_label(), p.trust_label());
    println!("Description  : {}", p.description);
    if let Some(a) = &p.arch {
        println!("Architecture : {a}");
    }
    if !p.depends.is_empty() {
        println!("Depends on   : {}", p.depends.join("  "));
    }
    if !p.opt_depends.is_empty() {
        println!("Optional deps:");
        for d in &p.opt_depends {
            match &d.description {
                Some(desc) => println!("  {}: {desc}", d.name),
                None => println!("  {}", d.name),
            }
        }
    }
    if let Some(url) = &p.homepage {
        println!("Homepage     : {url}");
    }
    if !p.licenses.is_empty() {
        println!("Licenses     : {}", p.licenses.join("  "));
    }
    if let Some(m) = &p.maintainer {
        println!("Maintainer   : {m}");
    }
    if p.size.download.is_some() || p.size.installed.is_some() {
        print!("Size         : ");
        if let Some(d) = p.size.download {
            print!("{} download", format_size(d));
        }
        if let Some(i) = p.size.installed {
            print!("  {} installed", format_size(i));
        }
        println!();
    }
    match &p.status {
        crate::package::InstallStatus::NotInstalled => println!("Status       : not installed"),
        crate::package::InstallStatus::Installed { version, reason } => {
            println!(
                "Status       : installed ({version}){}",
                reason_label(*reason)
            );
        }
    }
    if let Some(votes) = p.num_votes {
        println!("Votes        : {votes}");
    }
    if p.out_of_date {
        println!("WARNING      : flagged out-of-date upstream");
    }
}

fn reason_label(reason: Option<crate::package::InstallReason>) -> &'static str {
    match reason {
        Some(crate::package::InstallReason::Explicit) => ", explicit",
        Some(crate::package::InstallReason::Dependency) => ", as dependency",
        None => "",
    }
}

async fn install_cmd(
    registry: &Registry,
    cfg: &Config,
    names: &[String],
    yes: bool,
    reviewed: bool,
    dry_run: bool,
) -> Result<(), ForgeError> {
    if names.is_empty() {
        return Err(ForgeError::InvalidInput("no package specified".into()));
    }
    if reviewed {
        if let Some(aur) = registry.get(BackendId::Aur) {
            aur.as_any()
                .downcast_ref::<crate::backend::aur::AurBackend>()
                .map(|b| b.mark_many_reviewed(names.iter().cloned()));
        }
    }
    let actions = resolve_packages_to_actions(registry, names, ActionKind::Install).await?;
    run_confirmed(registry, cfg, actions, yes, dry_run).await
}

async fn remove_cmd(
    registry: &Registry,
    cfg: &Config,
    names: &[String],
    yes: bool,
    dry_run: bool,
) -> Result<(), ForgeError> {
    if names.is_empty() {
        return Err(ForgeError::InvalidInput("no package specified".into()));
    }
    // Removals always target pacman's local database (covers AUR too).
    let actions: Vec<crate::transaction::Action> = names
        .iter()
        .map(|n| crate::transaction::Action {
            kind: ActionKind::Remove,
            backend: BackendId::Pacman,
            id: n.clone(),
            name: n.clone(),
            version: None,
        })
        .collect();
    run_confirmed(registry, cfg, actions, yes, dry_run).await
}

async fn update_cmd(
    registry: &Registry,
    cfg: &Config,
    yes: bool,
    reviewed: bool,
    dry_run: bool,
) -> Result<(), ForgeError> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TxEvent>();
    let printer = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Some(line) = ev.to_log_line() {
                println!("{line}");
            }
        }
    });
    let sender: TxEventSender = tx.clone();
    let cancel = CancelToken::new();

    // 1) Refresh pacman databases through polkit (skipped for dry runs).
    if !dry_run {
        crate::backend::pacman::refresh_databases(sender.clone(), cancel.clone()).await?;
    }

    // 2) Collect "upgrade everything" actions for every available backend.
    let mut actions = vec![crate::backend::pacman::upgrade_all_action()];
    if let Some(flatpak) = registry.get(BackendId::Flatpak) {
        if flatpak.available().await {
            for scope_suffix in ["@user", "@system"] {
                actions.push(crate::transaction::Action {
                    kind: ActionKind::Upgrade,
                    backend: BackendId::Flatpak,
                    id: format!("__forge_flatpak_upgrade_all__{scope_suffix}"),
                    name: format!("All Flatpak updates ({scope_suffix})"),
                    version: None,
                });
            }
        }
    }

    // 3) Preview (pacman resolves the upgrade set), confirm, execute.
    let preview = registry.preview(&actions).await?;
    if preview.target_actions().is_empty() {
        println!("Nothing to update.");
        printer.abort();
        return Ok(());
    }
    print_preview(&preview);
    if dry_run {
        println!("dry run: nothing was executed.");
        printer.abort();
        return Ok(());
    }

    if reviewed {
        let aur_names: Vec<String> = preview
            .targets()
            .filter(|i| i.action.backend == BackendId::Aur)
            .map(|i| i.action.id.clone())
            .collect();
        if let Some(aur) = registry.get(BackendId::Aur) {
            if let Some(b) = aur
                .as_any()
                .downcast_ref::<crate::backend::aur::AurBackend>()
            {
                b.mark_many_reviewed(aur_names);
            }
        }
    }

    if !confirm(cfg, yes)? {
        println!("aborted.");
        printer.abort();
        return Ok(());
    }

    let summary = run_transaction(registry, &preview, &(tx as TxEventSender), cancel).await?;
    printer.abort();
    println!("{}", summary.message);
    if summary.succeeded {
        Ok(())
    } else {
        Err(ForgeError::Tool {
            tool: "transaction".into(),
            exit: None,
            message: summary.message,
        })
    }
}

async fn list_cmd(registry: &Registry, backend: Option<BackendId>) -> Result<(), ForgeError> {
    for b in registry.active().await {
        if let Some(want) = backend {
            if b.id() != want {
                continue;
            }
        }
        let pkgs = b.installed().await.unwrap_or_default();
        if pkgs.is_empty() {
            continue;
        }
        println!("== {} ==", b.id().label());
        for p in pkgs {
            println!("{:44} {}", truncate(&p.name, 44), p.version);
        }
    }
    Ok(())
}

/// Resolves user-typed names to concrete [`Action`]s by looking them up in
/// every active backend (first hit wins, repositories before AUR/Flatpak).
async fn resolve_packages_to_actions(
    registry: &Registry,
    names: &[String],
    kind: ActionKind,
) -> Result<Vec<crate::transaction::Action>, ForgeError> {
    let backends = registry.active().await;
    let order: [BackendId; 3] = [BackendId::Pacman, BackendId::Aur, BackendId::Flatpak];
    let mut actions = Vec::new();
    'outer: for name in names {
        crate::util::validate::ensure_valid_identifier(name)?;
        for bid in order {
            let Some(backend) = backends.iter().find(|b| b.id() == bid) else {
                continue;
            };
            if let Ok(Some(pkg)) = backend.info(name).await {
                let mut action = match kind {
                    ActionKind::Install => crate::transaction::Action::install(&pkg),
                    ActionKind::Upgrade => crate::transaction::Action::upgrade(&pkg),
                    ActionKind::Reinstall => crate::transaction::Action::reinstall(&pkg),
                    ActionKind::Remove => crate::transaction::Action::remove(&pkg),
                };
                action.id = pkg.id.clone();
                actions.push(action);
                continue 'outer;
            }
        }
        return Err(ForgeError::InvalidInput(format!(
            "'{name}' not found in any enabled source"
        )));
    }
    Ok(actions)
}

async fn run_confirmed(
    registry: &Registry,
    cfg: &Config,
    actions: Vec<crate::transaction::Action>,
    yes: bool,
    dry_run: bool,
) -> Result<(), ForgeError> {
    let preview = registry.preview(&actions).await?;
    print_preview(&preview);

    if dry_run {
        println!("dry run: nothing was executed.");
        return Ok(());
    }

    if matches_source(&preview, Source::Aur) {
        // The AUR review gate is enforced inside the backend at execute time;
        // remind CLI users here.
        if !yes {
            println!("\nNote: AUR installs require --reviewed unless disabled in config.");
        }
    }

    if !confirm(cfg, yes)? {
        println!("aborted.");
        return Ok(());
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TxEvent>();
    let printer = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Some(line) = ev.to_log_line() {
                println!("{line}");
            }
        }
    });
    let summary = run_transaction(registry, &preview, &tx, CancelToken::new()).await?;
    printer.abort();
    println!("{}", summary.message);
    if summary.succeeded {
        Ok(())
    } else {
        Err(ForgeError::Tool {
            tool: "transaction".into(),
            exit: None,
            message: summary.message,
        })
    }
}

fn matches_source(plan: &TransactionPreview, source: Source) -> bool {
    plan.items
        .iter()
        .any(|i| i.action.backend == source.backend())
}

fn print_preview(plan: &TransactionPreview) {
    use crate::transaction::Reason::*;
    println!("\nTransaction preview:");
    for item in &plan.items {
        let tag = match item.reason {
            Target => " ",
            Dependency => "+ dep",
            NoLongerNeeded => "- orphan",
            ConflictReplacement => "! conflict",
        };
        let versions = match (&item.current_version, &item.new_version) {
            (Some(c), Some(n)) => format!("{c} -> {n}"),
            (None, Some(n)) => n.clone(),
            (Some(c), None) => c.clone(),
            (None, None) => String::new(),
        };
        let size = item.download_size.map(format_size).unwrap_or_default();
        println!(
            "  [{tag:>8}] {:40} {:24} {:>10}",
            truncate(&item.title(), 40),
            truncate(&versions, 24),
            size
        );
    }
    if plan.total_download_size() > 0 {
        println!(
            "  Download size : {}",
            format_size(plan.total_download_size())
        );
    }
    if plan.total_installed_size() > 0 {
        println!(
            "  Installed size: {}",
            format_size(plan.total_installed_size())
        );
    }
    for w in &plan.warnings {
        println!("  warning: {w}");
    }
    if plan.needs_privileges {
        println!("  This operation requires authentication (polkit will ask for your password).");
    }
    println!();
}

/// Interactive y/N confirmation on terminals; `--yes` bypasses.
fn confirm(cfg: &Config, yes_flag: bool) -> Result<bool, ForgeError> {
    if yes_flag || !cfg.general.confirm_before_transaction {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Ok(false); // non-interactive without --yes: never guess
    }
    print!("Proceed? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    Ok(matches!(
        buf.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
