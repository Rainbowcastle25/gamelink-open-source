use std::{
    io,
    process::{ChildStdin, Command, Stdio},
    sync::Mutex,
};
use tauri::{AppHandle, Manager, State};

const WAIT_FOR_PARENT: &str = "--gamelink-wait-for-restart";
const FAILED: &str = "Could not restart GameLink. Try restarting again.";

// Keep the pipe open until Tauri has shut down the session and released the
// single-instance lock. It also closes automatically if the old process dies.
#[derive(Default)]
pub struct Handoff(Mutex<Option<ChildStdin>>);

pub fn wait_for_parent() {
    if std::env::args_os().nth(1).as_deref() == Some(WAIT_FOR_PARENT.as_ref())
        && io::copy(&mut io::stdin().lock(), &mut io::sink()).is_err()
    {
        std::process::exit(1);
    }
}

fn launch_then_exit(
    handoff: &Handoff,
    command: &mut Command,
    exit: impl FnOnce(),
) -> Result<(), &'static str> {
    let mut pipe = handoff.0.lock().map_err(|_| FAILED)?;
    if pipe.is_some() {
        return Ok(());
    }
    let mut child = command.stdin(Stdio::piped()).spawn().map_err(|_| FAILED)?;
    *pipe = child.stdin.take();
    exit();
    Ok(())
}

#[tauri::command]
pub fn restart_game_link(app: AppHandle, handoff: State<'_, Handoff>) -> Result<(), &'static str> {
    let env = app.env();
    let executable = tauri::process::current_binary(&env).map_err(|_| FAILED)?;
    let mut args = env.args_os.iter().skip(1).peekable();
    if args.peek().is_some_and(|arg| *arg == WAIT_FOR_PARENT) {
        args.next();
    }
    launch_then_exit(
        &handoff,
        Command::new(executable).arg(WAIT_FOR_PARENT).args(args),
        || app.exit(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_failure_leaves_the_app_running_and_restart_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let handoff = Handoff::default();
        let result = launch_then_exit(
            &handoff,
            &mut Command::new(dir.path().join("missing")),
            || panic!("the app would exit after a failed spawn"),
        );
        assert_eq!(result, Err(FAILED));
        assert!(handoff.0.lock().unwrap().is_none());
    }
}
