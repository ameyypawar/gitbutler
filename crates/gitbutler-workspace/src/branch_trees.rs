use anyhow::Result;
use but_core::RepositoryExt as _;
use but_ctx::{
    Context,
    access::{RepoExclusive, RepoShared},
};
use but_oxidize::{ObjectIdExt, OidExt};
use gitbutler_cherry_pick::GixRepositoryExt as _;

use crate::{legacy_target_base_oid, legacy_workspace_stack_heads};

/// A snapshot of the workspace at a point in time.
#[derive(Debug)]
pub struct WorkspaceState {
    /// The commit ids of the stack heads in the workspace.
    heads: Vec<gix::ObjectId>,
    /// The commit id of the workspace target.
    target: gix::ObjectId,
}

impl WorkspaceState {
    pub fn create(ctx: &Context, _perm: &RepoShared) -> Result<Self> {
        let repo = ctx.repo.get()?;
        let target_base_oid = legacy_target_base_oid(ctx)?;
        let head_oids = legacy_workspace_stack_heads(ctx, &repo, target_base_oid)?;
        Ok(Self::from_heads_and_target(&head_oids, target_base_oid))
    }

    pub fn from_heads_and_target(
        head_oids: &[gix::ObjectId],
        target_base_oid: gix::ObjectId,
    ) -> Self {
        WorkspaceState {
            heads: head_oids.to_vec(),
            target: target_base_oid,
        }
    }

    pub fn create_from_heads(
        ctx: &Context,
        _perm: &RepoShared,
        heads: &[gix::ObjectId],
    ) -> Result<Self> {
        let target_base_oid = legacy_target_base_oid(ctx)?;
        Ok(Self::from_heads_and_target(heads, target_base_oid))
    }
}

/// Update the uncommitted changes from one snapshot of the workspace and rebase
/// them on top of the new snapshot.
pub fn update_uncommitted_changes(
    ctx: &Context,
    old: WorkspaceState,
    new: WorkspaceState,
    perm: &mut RepoExclusive,
) -> Result<()> {
    let repo = &*ctx.repo.get()?;
    let uncommitted_changes = if ctx.settings.feature_flags.cv3 {
        None
    } else {
        #[expect(deprecated)]
        Some(repo.create_wd_tree(0)?)
    };

    update_uncommitted_changes_with_tree(ctx, old, new, uncommitted_changes, None, perm)
}

/// `old_uncommitted_changes` is `None` if the `safe_checkout` feature is toggled on in `ctx`
pub fn update_uncommitted_changes_with_tree(
    ctx: &Context,
    old: WorkspaceState,
    new: WorkspaceState,
    old_uncommitted_changes: Option<gix::ObjectId>,
    always_checkout: Option<bool>,
    _perm: &mut RepoExclusive,
) -> Result<()> {
    if let Some(worktree_id) = old_uncommitted_changes {
        let gix_repo = ctx.clone_repo_for_merging()?;
        #[expect(deprecated, reason = "checkout/index materialization boundary")]
        let repo = &*ctx.git2_repo.get()?;
        let mut new_uncommitted_changes =
            move_tree_between_workspaces(repo, &gix_repo, worktree_id, &old, &new)?;

        // If the new tree and old tree are the same, then we don't need to do anything
        if !new_uncommitted_changes.has_conflicts() && !always_checkout.unwrap_or(false) {
            let tree = new_uncommitted_changes.write_tree_to(repo)?.to_gix();
            if tree == worktree_id {
                return Ok(());
            }
        }

        repo.checkout_index(
            Some(&mut new_uncommitted_changes),
            Some(
                git2::build::CheckoutBuilder::new()
                    .force()
                    .remove_untracked(true)
                    .conflict_style_diff3(true),
            ),
        )?;
    } else {
        let gix_repo = ctx.clone_repo_for_merging()?;
        let old_tree_id = merge_workspace(&gix_repo, &old)?;
        let new_tree_id = merge_workspace(&gix_repo, &new)?;
        but_core::worktree::safe_checkout(
            old_tree_id,
            new_tree_id,
            &gix_repo,
            but_core::worktree::checkout::Options::default(),
        )?;
    }
    Ok(())
}

/// Take the changes on top of one workspace and return what they would look
/// like if they were on top of the new workspace.
fn move_tree_between_workspaces(
    repo: &git2::Repository,
    gix_repo: &gix::Repository,
    tree: gix::ObjectId,
    old: &WorkspaceState,
    new: &WorkspaceState,
) -> Result<git2::Index> {
    let old_workspace = merge_workspace(gix_repo, old)?;
    let new_workspace = merge_workspace(gix_repo, new)?;
    move_tree(repo, tree, old_workspace, new_workspace)
}

/// Cherry pick a tree from one base tree on to another, favoring the contents of the tree when conflicts occur
fn move_tree(
    repo: &git2::Repository,
    tree: gix::ObjectId,
    old_workspace: gix::ObjectId,
    new_workspace: gix::ObjectId,
) -> Result<git2::Index> {
    // Read: Take the diff between old_workspace and tree, and apply it on top
    //   of new_workspace
    let merge = repo.merge_trees(
        &repo.find_tree(old_workspace.to_git2())?,
        &repo.find_tree(tree.to_git2())?,
        &repo.find_tree(new_workspace.to_git2())?,
        None,
    )?;

    Ok(merge)
}

/// Octopus-merge the stack heads into a single tree (via gix, so adjacent hunks
/// that git2 would flag as conflicts merge cleanly).
///
/// The result is a faithful merge of the stack heads *only* — the workspace
/// target is never a merge input (except as the empty-workspace fallback). All
/// heads are merged against one shared base: the octopus merge-base (lowest
/// common ancestor) of the heads. Using a single base keeps the result
/// independent of head order — a gradually-lowered per-iteration base instead
/// lets a stale value from a divergent-base stack win or lose depending on
/// ordering.
///
/// Seeding from the target instead of the heads would inject target-only content
/// no stack carries, and — when a stack is based below the target — attribute the
/// target's extra commits as deletions by that stack, silently dropping them and
/// surfacing as phantom uncommitted changes. The same octopus is implemented in
/// `but_workspace::legacy::remerged_workspace_tree_v2`.
///
/// With no heads, the target tree is returned (an empty workspace is just the target).
pub fn merge_workspace(
    repo: &gix::Repository,
    workspace: &WorkspaceState,
) -> Result<gix::ObjectId> {
    let real_tree = |oid: gix::ObjectId| -> Result<gix::ObjectId> {
        let commit = repo.find_commit(oid)?;
        Ok(repo.find_real_tree(&commit, Default::default())?.detach())
    };

    let Some((&first_head, rest)) = workspace.heads.split_first() else {
        return real_tree(workspace.target);
    };

    // Octopus merge-base of all heads, used as the single base for every merge.
    let mut base_commit = first_head;
    for &head in rest {
        base_commit = repo.merge_base(base_commit, head)?.detach();
    }
    let base_tree = real_tree(base_commit)?;

    let (merge_options, conflict_kind) = repo.merge_options_fail_fast()?;
    let mut output = real_tree(first_head)?;

    for &head in rest {
        let mut merge = repo.merge_trees(
            base_tree,
            output,
            real_tree(head)?,
            repo.default_merge_labels(),
            merge_options.clone(),
        )?;

        if merge.has_unresolved_conflicts(conflict_kind) {
            anyhow::bail!("merge conflict when computing workspace tree");
        }
        output = merge.tree.write()?.detach();
    }

    Ok(output)
}

pub fn move_tree_has_conflicts(
    ctx: &Context,
    tree: gix::ObjectId,
    old_workspace: gix::ObjectId,
    new_workspace: gix::ObjectId,
) -> Result<bool> {
    #[expect(deprecated, reason = "tree merge/index materialization boundary")]
    let repo = &*ctx.git2_repo.get()?;
    Ok(move_tree(repo, tree, old_workspace, new_workspace)?.has_conflicts())
}
