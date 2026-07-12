use bstr::ByteSlice;

use crate::{command::util, utils::Sandbox};

fn find_uncommitted_cli_id(status: &serde_json::Value, path_contains: &str) -> Option<String> {
    status["uncommittedChanges"]
        .as_array()?
        .iter()
        .find(|change| {
            change["filePath"]
                .as_str()
                .map(|path| path.contains(path_contains))
                .unwrap_or(false)
        })
        .and_then(|change| change["cliId"].as_str().map(ToOwned::to_owned))
}

#[test]
fn discard_removes_selected_change() -> anyhow::Result<()> {
    let env = Sandbox::init_scenario_with_target_and_default_settings("one-stack");
    env.setup_metadata(&["A"]);

    env.file("src/discard-me.ts", "export const value = true;\n");

    let status = util::status_json(&env)?;
    let cli_id = find_uncommitted_cli_id(&status, "discard-me").expect("should find CLI ID");

    env.but(format!("discard {cli_id}")).assert().success();

    let status = util::status_json(&env)?;
    assert!(
        find_uncommitted_cli_id(&status, "discard-me").is_none(),
        "discarded file should no longer appear in uncommitted changes"
    );
    assert!(
        !env.projects_root().join("src/discard-me.ts").exists(),
        "discarding a new file should remove it from the worktree"
    );

    Ok(())
}

#[test]
fn discard_by_path_prefix_removes_only_matching_files() -> anyhow::Result<()> {
    let env = Sandbox::init_scenario_with_target_and_default_settings("one-stack");
    env.setup_metadata(&["A"]);

    env.file("path/to/first.txt", "first\n");
    env.file("path/to/second.txt", "second\n");
    env.file("path/other/third.txt", "third\n");

    // Regression: `but discard <path-prefix>` resolves to `CliId::PathPrefix`, which
    // used to hit an unimplemented `todo!()` and panic the CLI instead of discarding.
    env.but("discard path/to/").assert().success();

    let status = util::status_json(&env)?;
    assert!(
        find_uncommitted_cli_id(&status, "path/to/first").is_none(),
        "file under the discarded prefix should be gone"
    );
    assert!(
        find_uncommitted_cli_id(&status, "path/to/second").is_none(),
        "second file under the discarded prefix should be gone"
    );
    assert!(
        find_uncommitted_cli_id(&status, "path/other/third").is_some(),
        "files outside the prefix must be left untouched"
    );

    assert!(!env.projects_root().join("path/to/first.txt").exists());
    assert!(!env.projects_root().join("path/to/second.txt").exists());
    assert!(
        env.projects_root().join("path/other/third.txt").exists(),
        "unrelated file must remain in the worktree"
    );

    Ok(())
}

#[test]
fn discard_bare_path_prefix_leaves_stack_assigned_changes() -> anyhow::Result<()> {
    let env = Sandbox::init_scenario_with_target_and_default_settings("one-stack");
    env.setup_metadata(&["A"]);

    env.file("path/to/unassigned.txt", "u\n");
    env.file("path/to/assigned.txt", "a\n");

    // Assign one file under the prefix to stack A.
    env.but("rub path/to/assigned.txt A").assert().success();

    // A bare prefix resolves to unassigned hunks only, so this discards the
    // unassigned file and leaves the stack-assigned one untouched.
    env.but("discard path/to/").assert().success();

    assert!(
        !env.projects_root().join("path/to/unassigned.txt").exists(),
        "unassigned file under the prefix should be discarded"
    );
    assert!(
        env.projects_root().join("path/to/assigned.txt").exists(),
        "stack-assigned file under the prefix must be left (bare prefix = unassigned only)"
    );

    Ok(())
}

#[test]
fn concurrent_discard_to_independent_files_succeeds() -> anyhow::Result<()> {
    let env = Sandbox::init_scenario_with_target_and_default_settings("one-stack");
    env.setup_metadata(&["A"]);

    env.file("src/a/discard.ts", "export const a = true;\n");
    env.file("src/b/discard.ts", "export const b = true;\n");

    let status = util::status_json(&env)?;
    let id_a = find_uncommitted_cli_id(&status, "a/discard").expect("should find first CLI ID");
    let id_b = find_uncommitted_cli_id(&status, "b/discard").expect("should find second CLI ID");

    let child_a = util::but_std_cmd(&env, &format!("discard {id_a}")).spawn()?;
    let child_b = util::but_std_cmd(&env, &format!("discard {id_b}")).spawn()?;

    let out_a = child_a.wait_with_output()?;
    let out_b = child_b.wait_with_output()?;

    assert!(
        out_a.status.success(),
        "first discard failed: {}",
        out_a.stderr.as_bstr()
    );
    assert!(
        out_b.status.success(),
        "second discard failed: {}",
        out_b.stderr.as_bstr()
    );

    let status = util::status_json(&env)?;
    assert_eq!(
        status["uncommittedChanges"]
            .as_array()
            .map(|changes| changes.len())
            .unwrap_or(0),
        0,
        "both discarded files should be removed from the workspace"
    );

    Ok(())
}

#[test]
fn discard_reverts_simple_rename() -> anyhow::Result<()> {
    let env = Sandbox::init_scenario_with_target_and_default_settings("one-stack");
    env.setup_metadata(&["A"]);

    env.file("src/rename-source.ts", "export const source = true;\n");
    env.but("commit A -m 'seed rename source'")
        .assert()
        .success();

    std::fs::rename(
        env.projects_root().join("src/rename-source.ts"),
        env.projects_root().join("src/rename-target.ts"),
    )?;

    let status = util::status_json(&env)?;
    let cli_id =
        find_uncommitted_cli_id(&status, "rename-target").expect("should find renamed file CLI ID");

    env.but(format!("discard {cli_id}")).assert().success();

    assert!(
        env.projects_root().join("src/rename-source.ts").exists(),
        "discarding a rename should restore the source path"
    );
    assert!(
        !env.projects_root().join("src/rename-target.ts").exists(),
        "discarding a rename should remove the target path"
    );
    assert_eq!(
        env.invoke_git("status --porcelain"),
        "",
        "discarding a rename should leave a clean worktree"
    );

    Ok(())
}

#[test]
fn discard_rename_does_not_discard_unrelated_changes() -> anyhow::Result<()> {
    let env = Sandbox::init_scenario_with_target_and_default_settings("one-stack");
    env.setup_metadata(&["A"]);

    env.file("src/rename-source-only.ts", "export const source = 1;\n");
    env.but("commit A -m 'seed rename source only'")
        .assert()
        .success();

    std::fs::rename(
        env.projects_root().join("src/rename-source-only.ts"),
        env.projects_root().join("src/rename-target-only.ts"),
    )?;
    env.file("src/keep-me.ts", "export const keep = true;\n");

    let status_before = util::status_json(&env)?;
    let cli_id = find_uncommitted_cli_id(&status_before, "rename-target-only")
        .expect("should find renamed file CLI ID");

    env.but(format!("discard {cli_id}")).assert().success();

    assert!(
        env.projects_root()
            .join("src/rename-source-only.ts")
            .exists(),
        "discarding rename should restore source path"
    );
    assert!(
        !env.projects_root()
            .join("src/rename-target-only.ts")
            .exists(),
        "discard should remove renamed target path"
    );

    let status_after = util::status_json(&env)?;
    assert!(
        find_uncommitted_cli_id(&status_after, "rename-target-only").is_none(),
        "discarded renamed file should no longer appear in uncommitted changes"
    );
    assert!(
        find_uncommitted_cli_id(&status_after, "keep-me").is_some(),
        "discarding a rename should not discard unrelated uncommitted changes"
    );

    let git_status = env.invoke_git("status --porcelain");
    assert!(
        git_status.contains("src/keep-me.ts"),
        "expected unrelated uncommitted file to remain, got:\n{git_status}"
    );
    assert!(
        !git_status.contains("rename-target-only") && !git_status.contains("rename-source-only"),
        "rename paths should no longer be dirty, got:\n{git_status}"
    );

    Ok(())
}

#[test]
fn discard_the_whole_uncommitted_changes() -> anyhow::Result<()> {
    let env = Sandbox::init_scenario_with_target_and_default_settings("one-stack");
    env.setup_metadata(&["A"]);

    env.file("src/rename-source-only.ts", "export const source = 1;\n");
    env.but("commit A -m 'seed rename source only'")
        .assert()
        .success();

    std::fs::rename(
        env.projects_root().join("src/rename-source-only.ts"),
        env.projects_root().join("src/rename-target-only.ts"),
    )?;
    env.file("src/keep-me.ts", "export const keep = true;\n");

    env.but("discard zz").assert().success();

    assert!(
        env.projects_root()
            .join("src/rename-source-only.ts")
            .exists(),
        "discarding rename should restore source path"
    );
    assert!(
        !env.projects_root()
            .join("src/rename-target-only.ts")
            .exists(),
        "discard should remove renamed target path"
    );

    let status_after = util::status_json(&env)?;
    assert!(
        find_uncommitted_cli_id(&status_after, "rename-target-only").is_none(),
        "discarded renamed file should no longer appear in uncommitted changes"
    );
    assert!(
        find_uncommitted_cli_id(&status_after, "keep-me").is_none(),
        "the added file should be gone as well"
    );

    assert_eq!(
        env.invoke_git("status --porcelain"),
        "",
        "discarding a rename should leave a clean worktree"
    );

    Ok(())
}

#[test]
fn discarding_multiple_hunks_in_a_file_works() -> anyhow::Result<()> {
    let env = Sandbox::init_scenario_with_target_and_default_settings("one-stack");
    env.setup_metadata(&["A"]);

    let content = "1\n2\n3\n4\n5\n6\n7";
    let file_path = "src/some_file.txt";

    env.file(file_path, content);
    env.but("commit A -m 'seed rename source only'")
        .assert()
        .success();

    env.file(file_path, "a\nb\nc\n1\n2\n3\n4\n5\n6\n7\nd\ne\nf");
    env.but("discard zz").assert().success();

    assert!(
        env.projects_root().join("src/some_file.txt").exists(),
        "discarding multiple hunks should keep the tracked file present"
    );

    let content_after_discard = env.read_file(file_path)?;
    assert_eq!(content_after_discard, content);

    Ok(())
}
