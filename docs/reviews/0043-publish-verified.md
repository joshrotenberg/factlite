# 8.2 Publish and verify: self-review

This is a self-review. No independent review ran.

## What was done

- The user created the private repository and pushed `main` (`250bc6b`). Before
  pushing anything, a fetch showed `origin/main` equal to local `main`, with no
  unrelated history.
- Every local branch was confirmed to be an ancestor of `main`, so pushing
  them added names only. `git push origin --all` published 44 more branches.
  There are no tags.
- Remote and local branch refs were compared as sorted (hash, name) lists:
  identical, 45 branches.
- A fresh `--mirror` clone passed `git fsck --full` with 89 reachable commits,
  the same as locally. A fresh working clone has `main` at `250bc6b`, tree
  `3f1e186`, and `git ls-files -s` output identical to the local checkout. The
  asserted Rust workflow example runs from it.
- The archive, manifest and checksum were built from the remote mirror, not the
  local checkout, and the checksum was verified.

## CI

- First run on `main`: success. 181 tests passed and 1 was ignored, on
  ubuntu-latest with the pinned Rust 1.99.0. The three pod babashka tests skip
  on the runner because babashka is not installed.
- The user chose to pause the workflow while pushing branches. It was disabled
  for the push and re-enabled immediately afterwards, but GitHub created 32
  branch runs anyway, apparently processing the push events after
  re-enabling. 27 were cancelled and 5 finished successfully before cancellation
  reached them; none failed.
- The push trigger is now limited to `main`; pull requests cover branches. Old
  branch heads keep the previous workflow file, but are not pushed again.

## Limitations

- No license is selected.
- CI does not run the babashka tests.
