#![expect(
    deprecated,
    reason = "VirtualBranchesHandle should be replaced with ctx.workspace_* helpers"
)]

use anyhow::Result;
use gitbutler_stack::VirtualBranchesHandle;
use tempfile::TempDir;

use but_ctx::Context;

use crate::driverless;

fn command_ctx(name: &str) -> Result<(Context, TempDir)> {
    driverless::writable_context("workspace-commit.sh", name)
}

/// When two applied stacks have trees that conflict on the same file,
/// `remerged_workspace_tree_v2` (called by `update_workspace_commit`) detects the
/// gix merge conflict and marks the later stack as `in_workspace = false`.
/// With the fix in `remerged_workspace_commit_v2`, that evicted stack's head must
/// be excluded from the workspace commit's parent list.
///
/// Without the fix, the workspace commit tree would not contain the evicted stack's
/// changes but its head would still be a parent — causing phantom uncommitted changes
/// when diffing the workspace commit against its parents.
#[test]
fn conflicting_stacks_evicted_from_workspace_commit_parents() -> Result<()> {
    let (ctx, _temp_dir) = command_ctx("conflicting-stacks")?;

    let vb_state = VirtualBranchesHandle::new(ctx.project_data_dir());
    let stacks_before = vb_state.list_stacks_in_workspace()?;
    assert_eq!(
        stacks_before.len(),
        2,
        "precondition: 2 stacks in workspace"
    );

    // Rebuild the workspace commit through the legacy path.
    // remerged_workspace_tree_v2 iterates both stacks and merges each tree:
    //   - The first stack merges cleanly onto the target tree
    //   - The second stack conflicts (same file, different content) → in_workspace = false
    // remerged_workspace_commit_v2 (with our fix) then excludes the evicted stack
    // from the workspace commit's parent list.
    gitbutler_branch_actions::update_workspace_commit(&ctx, false)?;

    let vb_state = VirtualBranchesHandle::new(ctx.project_data_dir());

    // Exactly one of the two conflicting stacks should have been evicted.
    let stacks_after = vb_state.list_stacks_in_workspace()?;
    assert_eq!(
        stacks_after.len(),
        1,
        "Only the non-conflicting stack should remain in workspace"
    );
    let surviving_stack = &stacks_after[0];

    // The workspace commit must have exactly 1 parent: the surviving stack's head.
    let repo = ctx.repo.get()?;
    let ws_ref = repo.find_reference("refs/heads/gitbutler/workspace")?;
    let ws_commit = ws_ref.into_fully_peeled_id()?.object()?.try_into_commit()?;
    let parent_ids: Vec<_> = ws_commit.parent_ids().collect();

    assert_eq!(
        parent_ids.len(),
        1,
        "Workspace commit should have only the surviving stack as parent"
    );

    let surviving_head = surviving_stack.head_oid(&ctx)?;
    assert_eq!(
        parent_ids[0].detach(),
        surviving_head,
        "The only parent should be the surviving stack's head"
    );

    Ok(())
}

/// When two applied stacks modify adjacent but non-overlapping sections of the same
/// file, `merge_workspace` must produce a clean merge.
///
/// Stack A owns lines 1–5 and 11–15; Stack B owns lines 6–10.
/// A's top hunk immediately precedes B's hunk (adjacency from above) and B's hunk
/// immediately precedes A's bottom hunk (adjacency from below).
///
/// Before the fix, `merge_workspace` used git2's Myers diff which incorrectly flagged
/// these adjacent hunks as conflicting (`MergeConflict (-24)`), breaking every workspace
/// mutation (squash, reorder, etc.) that recomputed the workspace tree.
#[test]
fn merge_workspace_succeeds_with_adjacent_hunks_from_both_sides() -> Result<()> {
    let (ctx, _temp_dir) = command_ctx("adjacent-stacks")?;

    // Build the workspace commit so both stacks are properly registered.
    gitbutler_branch_actions::update_workspace_commit(&ctx, false)?;

    let vb_state = VirtualBranchesHandle::new(ctx.project_data_dir());
    let stacks = vb_state.list_stacks_in_workspace()?;
    assert_eq!(stacks.len(), 2, "both stacks should be in workspace");

    // Build a WorkspaceState from both stacks and call merge_workspace directly.
    // This is the exact function that was fixed from git2 to gix.
    let guard = ctx.shared_worktree_access();
    let workspace =
        gitbutler_workspace::branch_trees::WorkspaceState::create(&ctx, guard.read_permission())?;
    let gix_repo = ctx.clone_repo_for_merging()?;
    gitbutler_workspace::branch_trees::merge_workspace(&gix_repo, &workspace)?;

    Ok(())
}

/// The workspace tree is a faithful octopus merge of the stack heads — the
/// target is never a merge input. Two invariants follow, and both used to be
/// violated (each surfacing as phantom uncommitted changes):
///
/// 1. content that *is* in some head must survive, and
/// 2. content that is in *no* head must not appear.
///
/// `diverged-stacks` has three stacks forked at successively older upstream
/// commits (v1 -> v2 -> v3 = target), none editing `shared.txt`, so they carry
/// different inherited versions of it:
///
///   section A:  stack_a = ALPHA (v1)    stack_b = a1 (v2)     stack_c = a1 (v3)
///   section B:  stack_a = b1 (v1)       stack_b = BRAVO (v2)  stack_c = b1 (v3)
///
/// Octopus-merging the heads against their shared base v1 (lowest common
/// ancestor):
/// - A: stack_a keeps the old `ALPHA` while b/c advanced to `a1` -> `a1` wins.
/// - B: only stack_b changed it (to `BRAVO`) relative to v1     -> `BRAVO` wins.
///
/// Both are deterministic because every head merges against the *same* base v1
/// (see `merge_workspace`); a gradually-lowered base would make B order-dependent.
/// The original bug (target tree as the base) instead reverted A to `ALPHA`.
#[test]
fn merge_workspace_merges_stack_heads_only() -> Result<()> {
    let (ctx, _temp_dir) = command_ctx("diverged-stacks")?;

    let repo = ctx.repo.get()?;
    let target_oid = repo.rev_parse_single("current-target")?.detach();
    let head_oids: Vec<gix::ObjectId> = ["stack_a", "stack_b", "stack_c"]
        .iter()
        .map(|name| repo.rev_parse_single(*name).map(|id| id.detach()))
        .collect::<Result<_, _>>()?;

    let workspace = gitbutler_workspace::branch_trees::WorkspaceState::from_heads_and_target(
        &head_oids, target_oid,
    );

    let gix_repo = ctx.clone_repo_for_merging()?;
    let merged_tree_id = gitbutler_workspace::branch_trees::merge_workspace(&gix_repo, &workspace)
        .expect("stack heads should octopus-merge cleanly");

    let merged_tree = gix_repo.find_tree(merged_tree_id)?;
    assert_stack_heads_merged(&merged_tree)?;

    Ok(())
}

/// Invariant 2: a target-only file (here `new_upstream.txt`, which only exists
/// in `upstream-target`/v4, ahead of every stack) must NOT leak into the
/// workspace tree. Seeding the merge from the target would inject it even though
/// no applied stack carries it.
#[test]
fn merge_workspace_excludes_target_only_content() -> Result<()> {
    let (ctx, _temp_dir) = command_ctx("diverged-stacks")?;

    let repo = ctx.repo.get()?;
    // A target ahead of all stacks: v4 adds new_upstream.txt that no stack has.
    let target_oid = repo.rev_parse_single("upstream-target")?.detach();
    let head_oids: Vec<gix::ObjectId> = ["stack_a", "stack_b", "stack_c"]
        .iter()
        .map(|name| repo.rev_parse_single(*name).map(|id| id.detach()))
        .collect::<Result<_, _>>()?;

    let workspace = gitbutler_workspace::branch_trees::WorkspaceState::from_heads_and_target(
        &head_oids, target_oid,
    );

    let gix_repo = ctx.clone_repo_for_merging()?;
    let merged_tree_id = gitbutler_workspace::branch_trees::merge_workspace(&gix_repo, &workspace)
        .expect("stack heads should octopus-merge cleanly");

    let merged_tree = gix_repo.find_tree(merged_tree_id)?;
    for path in ["file_a.txt", "file_b.txt", "file_c.txt"] {
        assert!(
            merged_tree.lookup_entry_by_path(path)?.is_some(),
            "{path} should be in merged workspace tree"
        );
    }
    assert!(
        merged_tree
            .lookup_entry_by_path("new_upstream.txt")?
            .is_none(),
        "target-only file must not leak into a workspace tree built from stack heads"
    );

    Ok(())
}

/// Same invariants as `merge_workspace_merges_stack_heads_only`, but end-to-end
/// through `update_workspace_commit` -> `remerged_workspace_tree_v2`, asserting
/// on the actual `gitbutler/workspace` HEAD tree.
#[test]
fn update_workspace_commit_merges_stack_heads_only() -> Result<()> {
    let (ctx, _temp_dir) = command_ctx("diverged-stacks")?;

    gitbutler_branch_actions::update_workspace_commit(&ctx, false)?;

    let repo = ctx.repo.get()?;
    let ws_tree = repo
        .find_reference("refs/heads/gitbutler/workspace")?
        .into_fully_peeled_id()?
        .object()?
        .try_into_commit()?
        .tree()?;

    assert_stack_heads_merged(&ws_tree)?;

    Ok(())
}

/// Assert the merged `diverged-stacks` tree is the octopus of the three stack
/// heads against their shared base v1: every stack's file present, section A
/// advanced to `a1` (old stack_a's `ALPHA` did not win), section B is stack_b's
/// `BRAVO` (the only head that changed it relative to the base).
///
/// Both section assertions are deterministic because all heads merge against the
/// same base (the octopus merge-base) — a per-iteration base would make B depend
/// on stack order. Section A also guards the original bug (target tree as the
/// base reverted A to `ALPHA`).
fn assert_stack_heads_merged(tree: &gix::Tree<'_>) -> Result<()> {
    for path in ["file_a.txt", "file_b.txt", "file_c.txt"] {
        assert!(
            tree.clone().lookup_entry_by_path(path)?.is_some(),
            "{path} should be in merged workspace tree"
        );
    }

    let shared = tree
        .clone()
        .lookup_entry_by_path("shared.txt")?
        .expect("shared.txt should exist")
        .object()?;
    let contents = std::str::from_utf8(&shared.data)?;
    assert!(
        contents.contains("line a1") && !contents.contains("ALPHA"),
        "section A should advance to a1 (old stack must not revert it), got:\n{contents}"
    );
    assert!(
        contents.contains("BRAVO ONE"),
        "section B should deterministically be stack_b's BRAVO (single octopus base), got:\n{contents}"
    );

    Ok(())
}
