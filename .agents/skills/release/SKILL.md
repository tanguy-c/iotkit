---
name: release
description: Prepare an iotkit release: check the CHANGELOG's Unreleased section, pick the version, write the summary, update the hardware test table, then open the release pull request with tools/release.sh. Use when the maintainer asks to cut, prepare or publish a release.
---

Work on `main`, up to date with `origin/main`. Show the maintainer each text you write before step 6.

1. **Check `## [Unreleased]`** in `CHANGELOG.md` against `git log v<last>..origin/main` (the last
   tag: `git describe --tags --abbrev=0`). Add an entry for each change users can see that has
   none, in the section that fits (Added, Changed, Removed, Fixed, Security). Follow AGENTS.md,
   "Writing for people".
2. **Pick the version.** Before 1.0, any **Breaking:** entry raises the minor version (0.5.1 to
   0.6.0); otherwise fixes alone raise the patch (0.5.1 to 0.5.2), and new features the minor.
   Propose it, with the reason, and let the maintainer decide.
3. **Check the upgrade steps.** Each **Breaking:** entry needs its steps in
   `docs/src/upgrading.md`, under the new version.
4. **Write the summary**: one or two sentences right under `## [Unreleased]`, before its first
   `###`. Say who should update and what they have to do, as in the past releases' summaries.
   `tools/release.sh` refuses to run without it.
5. **Update "Tested on hardware"** in `docs/src/introduction.md`. Ask the maintainer which boards
   they ran this release on and what they covered; set those rows' version to the new one. Change
   no row they did not test.
6. **Open the release pull request**: `tools/release.sh <version>`. It commits your uncommitted
   edits to `CHANGELOG.md` and `docs/` with the version bump, and refuses any other change.
7. **Remind the maintainer** to test the release branch on a C6 board before merging. Merging
   publishes: `.github/workflows/release.yml` tags the merge and creates the GitHub release from
   `tools/release-notes.sh`.
