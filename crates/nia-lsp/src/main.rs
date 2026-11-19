// SPDX-License-Identifier: GPL-3.0-or-later
//! Nia language server.

mod analysis;
mod document;
mod editing;
mod highlight;
mod project;
mod rename;
mod server;
mod structure;
mod workspace;

nia_alloc::global_allocator!();

fn timing_mode() -> nia_timing::TimingMode {
    static MODE: std::sync::OnceLock<nia_timing::TimingMode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("NIA_LSP_TIMINGS").as_deref() {
        Ok("detail") => nia_timing::TimingMode::Detail,
        Ok("summary") => nia_timing::TimingMode::Summary,
        _ => nia_timing::TimingMode::Off,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args_os().skip(1);
    let resource_root: Option<std::path::PathBuf> = match args.next() {
        None => None,
        Some(arg) if arg == "--version" && args.next().is_none() => {
            println!("nia-lsp {}", nia_compat::COMPILER_VERSION);
            return Ok(());
        }
        Some(arg) if arg == "--resource-root" => {
            let path = args.next().ok_or("missing --resource-root path")?;
            if args.next().is_some() {
                return Err("usage: nia-lsp [--resource-root PATH]".into());
            }
            Some(path.into())
        }
        _ => return Err("usage: nia-lsp [--resource-root PATH]".into()),
    };
    let executable = std::env::current_exe()?;
    let request = match resource_root {
        Some(root) => nia_toolchain::ToolchainLayoutRequest::explicit(executable, root),
        None => nia_toolchain::ToolchainLayoutRequest::installed(executable),
    };
    let toolchain = nia_toolchain::ToolchainLayout::resolve(request)?;
    let mode = timing_mode();
    if mode.enabled() {
        let options =
            nia_timing::TimingOptions::new(mode).with_format(nia_timing::TimingFormat::Json);
        nia_timing::collect_to_stderr(options, || server::run(toolchain))
    } else {
        server::run(toolchain)
    }
}
