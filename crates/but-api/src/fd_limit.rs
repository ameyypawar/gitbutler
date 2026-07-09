//! Raise the process's open-file-descriptor soft limit at startup.
//!
//! macOS ships a default `RLIMIT_NOFILE` soft limit of 256, which a large
//! rebase across many stacks can exhaust with `EMFILE` ("too many open
//! files"): each `gix`/`git2` repository memory-maps a potentially large
//! number of pack files on first object access, and several repositories can
//! be open at once. Hitting that limit mid-rebase can leave the workspace in
//! an inconsistent state, so raise the soft limit toward the hard limit up
//! front to avoid the crash entirely (#14260).

/// The soft `RLIMIT_NOFILE` we try to raise to. Capped rather than unbounded so
/// we don't request an enormous descriptor table on hosts with a very high
/// hard limit; this matches the `ulimit -n 65536` workaround that resolves the
/// issue in practice.
#[cfg(unix)]
const TARGET_NOFILE: u64 = 65536;

/// Raise the open-file soft limit toward the hard limit (up to `TARGET_NOFILE`).
///
/// Best-effort and safe to call once at startup: it never *lowers* the limit,
/// treats any failure as non-fatal (the process simply keeps the OS default),
/// and is a no-op on non-Unix platforms (Windows has no comparably low
/// default).
pub fn raise_soft_limit() {
    #[cfg(unix)]
    match rlimit::increase_nofile_limit(TARGET_NOFILE) {
        Ok(limit) => tracing::debug!("raised open-file soft limit to {limit}"),
        Err(err) => tracing::warn!("could not raise the open-file soft limit: {err}"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use rlimit::Resource;

    // We assert only the safety invariant (never lowers), not that the target
    // is reached: the OS-permitted maximum is platform-specific and, on macOS,
    // is `kern.maxfilesperproc` - which is well below `RLIM_INFINITY`, so the
    // achieved soft limit can legitimately be below `TARGET_NOFILE` even when
    // the reported hard limit is unlimited.
    #[test]
    fn raise_soft_limit_runs_and_never_lowers() {
        let (soft_before, _hard) = Resource::NOFILE.get().expect("read NOFILE limit");

        // The public helper is best-effort and must not panic.
        super::raise_soft_limit();

        let (soft_after, _hard) = Resource::NOFILE.get().expect("read NOFILE limit");
        assert!(
            soft_after >= soft_before,
            "raise_soft_limit must never lower the limit: {soft_after} < {soft_before}"
        );

        // Asking for less than the current soft limit is a no-op - we only ever
        // grow the descriptor table, never shrink it.
        if soft_before > 1 {
            let requested_lower =
                rlimit::increase_nofile_limit(soft_before - 1).expect("increase_nofile_limit");
            assert!(
                requested_lower >= soft_before,
                "requesting a lower target must not shrink the limit: {requested_lower} < {soft_before}"
            );
        }
    }
}
