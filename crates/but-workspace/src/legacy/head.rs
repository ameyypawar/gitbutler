//! Functions relate to the GitButler workspace head

use anyhow::Result;
use but_core::RepositoryExt;
use but_ctx::Context;
use gitbutler_cherry_pick::GixRepositoryExt as _;
use gitbutler_repo::{SignaturePurpose, commit_without_signature_gix, signature_gix};
use gitbutler_stack::{Stack, VirtualBranchesHandle};
use gix::merge::tree::TreatAsUnresolved;
use tracing::instrument;

const WORKSPACE_HEAD: &str = "Workspace Head";

/// Merges the tree of the workspace with the tree of the worktree, agnostic to which branch HEAD is pointing to
pub fn merge_worktree_with_workspace<'a>(
    ctx: &Context,
    gix_repo: &'a gix::Repository,
) -> Result<(gix::merge::tree::Outcome<'a>, TreatAsUnresolved)> {
    let mut head = gix_repo.head()?;

    // The uncommitted changes
    #[expect(deprecated)]
    let workdir_tree = gix_repo.create_wd_tree(0)?;

    // The tree of where the gitbutler workspace is at
    let workspace_tree = gix_repo
        .find_commit(super::remerged_workspace_commit_v2(ctx)?)?
        .tree_id()?
        .detach();

    let (merge_options_fail_fast, _conflict_kind) =
        gix_repo.merge_options_no_rewrites_fail_fast()?;

    let conflict_kind = TreatAsUnresolved::git();
    let outcome = gix_repo.merge_trees(
        head.peel_to_commit()?.tree_id()?,
        workdir_tree,
        workspace_tree,
        gix_repo.default_merge_labels(),
        merge_options_fail_fast.with_fail_on_conflict(Some(conflict_kind)),
    )?;
    Ok((outcome, conflict_kind))
}

/// Merge all currently stored stacks together into a new tree and return `(merged_tree, stacks, target_commit)` id accordingly.
/// `gix_repo` should be optimised for merging.
pub fn remerged_workspace_tree_v2(
    ctx: &Context,
    repo: &gix::Repository,
) -> Result<(gix::ObjectId, Vec<Stack>, gix::ObjectId)> {
    let mut vb_state = VirtualBranchesHandle::new(ctx.project_data_dir());
    let target_base_oid = ctx.persisted_default_target()?.sha;
    let mut stacks: Vec<Stack> = vb_state.list_stacks_in_workspace()?;

    let real_tree = |oid: gix::ObjectId| -> Result<gix::ObjectId> {
        let commit = repo.find_commit(oid)?;
        Ok(repo.find_real_tree(&commit, Default::default())?.detach())
    };

    // Resolve all in-workspace stack heads up front so the shared merge base can
    // be computed before merging.
    let head_oids = stacks
        .iter()
        .map(|stack| stack.head_oid(ctx))
        .collect::<Result<Vec<_>>>()?;

    // Octopus-merge the stack heads only — never the target. Seeding from the
    // target would pull in target-only content no stack carries, and (when a
    // stack is based below the target) attribute the target's extra commits as
    // deletions by that stack, silently dropping them and surfacing as phantom
    // uncommitted changes. The same octopus lives in
    // gitbutler_workspace::branch_trees::merge_workspace.
    //
    // All heads merge against ONE shared base — the octopus merge-base (lowest
    // common ancestor) of the heads — so the result is independent of stack
    // order. A gradually-lowered per-iteration base instead lets a stale value
    // from a divergent-base stack win or lose depending on ordering.
    let shared_base_tree = match head_oids.split_first() {
        None => None,
        Some((&first, rest)) => {
            let mut base = first;
            for &head in rest {
                base = repo.merge_base(base, head)?.detach();
            }
            Some(real_tree(base)?)
        }
    };

    let (merge_options_fail_fast, conflict_kind) = repo.merge_options_fail_fast()?;
    let mut acc: Option<gix::ObjectId> = None;
    for (stack, &stack_head_oid) in stacks.iter_mut().zip(&head_oids) {
        let branch_tree_id = real_tree(stack_head_oid)?;

        let Some(current_tree) = acc else {
            // First applied stack seeds the merge.
            acc = Some(branch_tree_id);
            continue;
        };

        let base_tree = shared_base_tree.expect("set whenever there is at least one head");
        let mut merge = repo.merge_trees(
            base_tree,
            current_tree,
            branch_tree_id,
            repo.default_merge_labels(),
            merge_options_fail_fast.clone(),
        )?;

        if !merge.has_unresolved_conflicts(conflict_kind) {
            acc = Some(merge.tree.write()?.detach());
        } else {
            // This branch should have already been unapplied during the "update" command but for some reason that failed
            tracing::warn!(
                "Merge conflict between {:?} and the workspace",
                stack.name()
            );
            stack.in_workspace = false;
            vb_state.set_stack(stack.clone())?;
        }
    }

    // No applied stacks (or all evicted): the workspace is just the target tree.
    let workspace_tree_id = match acc {
        Some(tree) => tree,
        None => real_tree(target_base_oid)?,
    };
    Ok((workspace_tree_id, stacks, target_base_oid))
}

/// Creates and returns a merge commit of all active branch heads.
///
/// This is the base against which we diff the working directory to understand
/// what files have been modified.
///
/// This should be used to update the `gitbutler/workspace` ref with, which is usually
/// done from `update_workspace_commit()`, after any of its input changes.
/// This is namely the conflicting state, or any head of the virtual branches.
#[instrument(level = "debug", skip(ctx))]
pub fn remerged_workspace_commit_v2(ctx: &Context) -> Result<gix::ObjectId> {
    let repo = ctx.clone_repo_for_merging()?;
    let (workspace_tree_id, stacks, target_commit) = remerged_workspace_tree_v2(ctx, &repo)?;

    let committer = signature_gix(SignaturePurpose::Committer);
    let author = signature_gix(SignaturePurpose::Author);
    let mut heads = stacks
        .iter()
        .filter(|stack| stack.in_workspace)
        .filter_map(|stack| stack.head_oid(ctx).ok())
        .collect::<Vec<_>>();

    if heads.is_empty() {
        heads = vec![target_commit]
    }

    let workspace_head_id = commit_without_signature_gix(
        &repo,
        None,
        author,
        committer,
        WORKSPACE_HEAD.into(),
        workspace_tree_id,
        &heads,
        None,
    )?;
    Ok(workspace_head_id)
}
