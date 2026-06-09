// tripatch - Triptych (AZSys) translation patcher and toolkit.
//
// Two audiences, one binary:
//   - Patch authors use the `extract` and `build` subcommands from a terminal.
//   - End users just double-click the .exe: with no arguments it opens native
//     Windows dialogs to pick a patch .json and the game folder, then installs.

mod asb;
mod azsys;
mod error;
mod patch;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use error::PatchError;

#[derive(Parser)]
#[command(
    name = "tripatch",
    version,
    about = "Triptych script.arc translation patcher",
    long_about = "Triptych (AZSys engine) translation toolkit.\n\n\
                  Run with no arguments to install a patch with native file \
                  pickers. Use the subcommands to author patches."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    // Dump all translatable text from an original script.arc into a JSON file.
    Extract {
        // Path to the original script.arc.
        #[arg(short, long)]
        input: PathBuf,
        // Where to write the texts JSON.
        #[arg(short, long, default_value = "texts.json")]
        output: PathBuf,
    },
    // Apply a translation JSON to a game install, rebuilding script.arc.
    Build {
        // Path to the patch JSON.
        #[arg(short, long)]
        input: PathBuf,
        // Game folder (or the script.arc path) to patch.
        #[arg(short, long)]
        output: PathBuf,
        // Decrypt and re-parse every script in the result as a self check.
        #[arg(long)]
        verify: bool,
    },
    // Install a patch using native file/folder pickers (same as no arguments).
    Install,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Extract { input, output }) => cmd_extract(&input, &output),
        Some(Command::Build {
            input,
            output,
            verify,
        }) => cmd_build(&input, &output, verify),
        Some(Command::Install) | None => cmd_install(),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[!] {e}");
            // When launched from a double-click there is no terminal to read
            // the message, so hold the window open on failure too.
            if cli_was_double_clicked() {
                pause();
            }
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

fn cmd_extract(input: &Path, output: &Path) -> Result<(), PatchError> {
    // The source for extraction is the original script.arc; back it up so the
    // pristine bytes survive even if the original is later patched in place.
    let source_label = input
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "script.arc".to_string());
    let target = patch::Target {
        original: input.to_path_buf(),
        backup: input
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("backup")
            .join(&source_label),
    };
    let arc_bytes = patch::ensure_backup(&target)?;
    println!("[+] source: {}", target.original.display());
    println!("[+] backup: {}", target.backup.display());

    let stats = patch::extract(&arc_bytes, &source_label, output)?;
    for w in &stats.warnings {
        println!("[!] {w}");
    }
    println!(
        "[+] {} text units from {} scripts -> {}",
        stats.total_units,
        stats.file_count,
        output.display()
    );
    Ok(())
}

fn cmd_build(input: &Path, output: &Path, verify: bool) -> Result<(), PatchError> {
    let doc = patch::load_document(input)?;
    let source = patch::source_name(&doc.meta);
    let game_dir = patch::game_dir_from_output(output, &source);
    run_install(&doc, &game_dir, verify)
}

fn cmd_install() -> Result<(), PatchError> {
    println!("Triptych patch installer");
    println!("Select the patch .json file...");
    let json_path = rfd::FileDialog::new()
        .add_filter("Patch JSON", &["json"])
        .set_title("Select the patch (.json)")
        .pick_file()
        .ok_or_else(|| PatchError::Io("no patch file selected".into()))?;

    let doc = patch::load_document(&json_path)?;
    let source = patch::source_name(&doc.meta);

    println!("Select the game folder (the one containing {source})...");
    let game_dir = rfd::FileDialog::new()
        .set_title(format!("Select the game folder (must contain {source})"))
        .pick_folder()
        .ok_or_else(|| PatchError::Io("no game folder selected".into()))?;

    // Validate the game by the presence of the source file.
    let target = patch::resolve_target(&game_dir, &source);
    if !target.original.exists() && !target.backup.exists() {
        let err = PatchError::Io(format!(
            "{} not found in {} - that does not look like the game folder",
            source,
            game_dir.display()
        ));
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Wrong folder")
            .set_description(err.to_string())
            .show();
        return Err(err);
    }

    let result = run_install(&doc, &game_dir, false);
    match &result {
        Ok(()) => {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Info)
                .set_title("Patch installed")
                .set_description(format!(
                    "The patch was installed successfully.\n\nGame: {}",
                    game_dir.display()
                ))
                .show();
        }
        Err(e) => {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Patch failed")
                .set_description(e.to_string())
                .show();
        }
    }
    pause();
    result
}

// Shared install path used by both `build` and the GUI installer.
fn run_install(doc: &patch::Document, game_dir: &Path, verify: bool) -> Result<(), PatchError> {
    let (stats, target) = patch::install(doc, game_dir)?;
    println!("[+] backup: {}", target.backup.display());
    println!(
        "[+] {} translated units, {} scripts modified",
        stats.translated_units, stats.scripts_modified
    );
    println!("[+] wrote {}", target.original.display());

    if verify {
        let bytes = std::fs::read(&target.original)?;
        let n = patch::verify(&bytes)?;
        println!("[+] verify OK: {n} scripts decrypt and parse");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Double-click helpers
// ---------------------------------------------------------------------------

// Heuristic: a process started by double-clicking in Explorer has no console
// arguments and an interactive window we should keep open.
fn cli_was_double_clicked() -> bool {
    std::env::args().count() <= 1
}

// Hold the window open until the user presses Enter.
fn pause() {
    print!("\nPress Enter to close...");
    let _ = std::io::stdout().flush();
    let mut buf = [0u8; 1];
    let _ = std::io::stdin().read(&mut buf);
}
