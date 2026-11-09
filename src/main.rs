//! fullscreen-ws daemon / CLI.

use fullscreen_ws::{Engine, Error, Settings, acquire_instance_lock};
use wayland_client::Connection;

const HELP: &str = "\
fullscreen-ws - give fullscreen/maximized windows their own COSMIC workspace

Usage: fullscreen-ws [OPTION]

With no option, runs as a daemon (normally started by systemd --user).

Options:
  --list               Print workspaces and windows, then exit
  --toggle <APP_ID>    Debug: fullscreen a matching window for 2s, then exit
  --move <APP_ID> <WS> Move a matching window to the workspace named WS
  -h, --help           Show this help
  -V, --version        Show version

Environment:
  FSWS_FULLSCREEN_ONLY=1   React to fullscreen only, not maximize
  FSWS_DEBUG=1             Verbose logging
";

fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().collect();
    let settings = Settings::new(
        true,
        std::env::var_os("FSWS_FULLSCREEN_ONLY").is_some(),
        std::env::var_os("FSWS_DEBUG").is_some(),
    );
    match args.get(1).map(String::as_str) {
        Some("-h" | "--help") => print!("{HELP}"),
        Some("-V" | "--version") => println!("fullscreen-ws {}", env!("CARGO_PKG_VERSION")),
        Some("--list") => Engine::new(Connection::connect_to_env()?, settings)?.print_list()?,
        Some("--toggle") => {
            let needle = args.get(2).ok_or("--toggle needs an app id")?;
            Engine::new(Connection::connect_to_env()?, settings)?.toggle_fullscreen(needle)?;
        }
        Some("--move") => {
            let (Some(needle), Some(ws)) = (args.get(2), args.get(3)) else {
                return Err("usage: --move <APP_ID> <WORKSPACE>".into());
            };
            Engine::new(Connection::connect_to_env()?, settings)?.move_window(needle, ws)?;
        }
        None => {
            let _lock = acquire_instance_lock()?;
            Engine::new(Connection::connect_to_env()?, settings)?.run()?;
        }
        Some(other) => return Err(format!("unknown option '{other}', see --help").into()),
    }
    Ok(())
}
