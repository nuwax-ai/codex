//! Keep Git's transport helpers inside the lifetime of a startup sync.

use std::os::unix::process::CommandExt as _;
use std::process::Command;

// A death signal on Git alone does not reach its remote-http/ssh children.
// Keep a small supervisor alive in the same process group until Git exits;
// parent death interrupts the shell's wait and kills the whole group.
const SUPERVISOR: &str = r#"
trap 'kill -KILL -$$' TERM
"$@" &
git_pid=$!
wait "$git_pid"
exit $?
"#;

pub(super) fn supervised_command(command: &Command) -> Command {
    // The program and every argument stay positional, including paths with
    // spaces or shell metacharacters. Only this fixed script is interpreted.
    let mut supervised = codex_utils_process::background_command("/bin/sh");
    supervised
        .args(["-c", SUPERVISOR, "codex-startup-git"])
        .arg(command.get_program())
        .args(command.get_args());
    if let Some(cwd) = command.get_current_dir() {
        supervised.current_dir(cwd);
    }
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                supervised.env(key, value);
            }
            None => {
                supervised.env_remove(key);
            }
        }
    }
    let parent_pid = unsafe { libc::getpid() };
    // SAFETY: pre_exec calls only the async-signal-safe process-group/prctl
    // helpers, before the supervisor can launch Git or any transport helper.
    unsafe {
        supervised.pre_exec(move || {
            codex_utils_pty::process_group::set_process_group()?;
            codex_utils_pty::process_group::set_parent_death_signal(parent_pid)
        });
    }
    supervised
}
