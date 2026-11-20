# Release-note fragments (host)

Don't edit `rust/RELEASE_NOTES.md` on a topic branch. Write `rust/release/notes.d/<topic>.md` instead, one file per topic, so parallel branches never conflict on the notes.

A fragment is a list of user-facing bullets in the style of `RELEASE_NOTES.md`: what changes for the user, measured numbers only, and no "Experimental" label (what our devices can't prove is marked in the status table and the hardware matrix only). An optional `# Topic` title line and HTML comments are dropped; anything else that isn't a `- ` bullet or an indented continuation line fails the assembly.

```markdown
# Encoder focus
- **Sharper text where you look.** ... Measured on the RX 7900 XT: ...
```

A merged pull request from a contributor (anyone but RamazanKara) always gets a fragment, written when the pull request is merged, whose bullet ends with the contributor's credit in bold: their name, GitHub login and the pull request.

```markdown
- **Dual screen.** ... Thanks to **Umberto Marotta** (@umbertomarotta, #75).
```

At release time `python rust/release/bump.py X.Y.Z` moves every fragment, in file name order, under `## Unreleased` and from there into `## New in X.Y.Z`, then deletes the fragments. `python rust/release/fragments.py` does the first step alone (to collect them between releases). This README stays.
