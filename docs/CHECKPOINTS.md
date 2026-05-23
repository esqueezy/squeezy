# Checkpoints

Squeezy creates local checkpoints around mutating tools so recent agent changes can be inspected, undone, or reverted without relying on the user's primary Git history.

Checkpoint state is stored under `.squeezy/checkpoints/` inside the workspace. The shadow Git repository stores before/after trees and `journal.jsonl` stores checkpoint metadata. Checkpoint refs keep those trees reachable until retention cleanup removes them.

## Protected Tools

Checkpoints are attached to mutating local tools:

- `write_file`
- `shell`

Read-only tools do not create checkpoints. A tool call that leaves the workspace unchanged does not create a checkpoint.

## Inspecting Checkpoints

Use `checkpoint_list` to list recent checkpoints. The response includes `journal_warnings` when malformed journal lines were ignored during recovery.

Use `checkpoint_show` with a `checkpoint_id` to inspect one checkpoint, including file paths, status, hashes, patch text for text files, binary markers, skipped files, and coverage warnings.

In the TUI:

- `/checkpoints` lists checkpoints.
- `/checkpoint <checkpoint_id>` shows one checkpoint.

## Undo And Revert

Use `checkpoint_undo` to roll back the latest checkpoint.

Use `checkpoint_revert` with exactly one of:

- `group_id` to revert all checkpoints from one turn or tool group.
- `checkpoint_id` to revert one checkpoint.

Rollback responses include:

- `mode`: rollback mode used.
- `planned_files`: number of protected files considered for rollback.
- `restored_files`: files restored to their previous content.
- `deleted_files`: files removed because they were added by the checkpoint.
- `conflicts`: files left untouched because the current content no longer matches the checkpoint's after-hash, or because required checkpoint objects are missing.
- `applied`: whether any rollback writes were attempted.
- `skipped`: whether no matching checkpoint was found.

## Rollback Modes

Rollback defaults to `atomic`.

`atomic` preflights every protected file in the selected checkpoint set. If any conflict is found, no file is changed.

`best_effort` restores clean files and leaves conflicting files untouched. Conflicts are still reported and the tool returns a stale result so the caller can decide what to do next.

Grouped rollbacks are applied in reverse checkpoint order, so a sequence of agent edits to the same file can be reverted back to the state before the group.

## Large And Binary Files

Binary files at or below the checkpoint size limit are restorable, but their patch text is omitted.

Files larger than 2 MiB are not stored in checkpoint trees. They are reported in `skipped_files`, and the checkpoint includes a `coverage_warnings` entry. Rollback will not restore skipped large files.

## Shell Coverage Warnings

Checkpoints only protect files inside the workspace. Shell commands can still mutate paths outside the workspace. Squeezy adds a coverage warning for obvious mutating shell commands that reference absolute paths or parent-directory traversal, such as `touch /tmp/file` or `rm ../file`.

The warning is advisory. It does not block the command and it does not make outside-workspace files restorable.

## Retention And Recovery

Checkpoint retention defaults to 7 days. Cleanup removes expired checkpoint journal entries and deletes their shadow Git refs, then prunes unreachable shadow Git objects.

Journal recovery ignores malformed JSONL lines and counts them as warnings. Rollback treats missing required checkpoint objects as conflicts and leaves current workspace content untouched.
