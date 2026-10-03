use super::*;

#[test]
fn exhausted_pipe_budget_prevents_spawning_another_probe_and_releases_reservations() {
    let leases: Vec<_> = (0..MAX_PIPE_READERS)
        .map(|_| PipeReaderLease::reserve().unwrap())
        .collect();
    let error = bounded_output(
        &mut Command::new("this-command-must-never-be-spawned"),
        Duration::from_secs(1),
        1024,
        1024,
    )
    .unwrap_err();
    assert!(error.contains("pipe reader budget exhausted"));
    drop(leases);
    assert!(PipeReaderLease::reserve().is_ok());
}

#[cfg(unix)]
#[test]
fn descendant_owned_pipes_time_out_without_unbounded_reader_growth() {
    for _ in 0..MAX_PIPE_READERS / 2 {
        let error = bounded_output(
            Command::new("sh").args(["-c", "sleep 3 & exit 0"]),
            Duration::from_millis(20),
            1024,
            1024,
        )
        .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
    }
    let error = bounded_output(
        &mut Command::new("this-command-must-never-be-spawned"),
        Duration::from_secs(1),
        1024,
        1024,
    )
    .unwrap_err();
    assert!(error.contains("pipe reader budget exhausted"));
}
