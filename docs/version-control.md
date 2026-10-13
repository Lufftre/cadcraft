# Drawings in git

> **Status:** step 1 of 4 (readable diffs, a review summary, compare with a branch in the app) · **Last updated:** 2026-10-13

CADCraft drawings can live in a git repository with readable history: `git diff`, `git log -p`
and pull requests show which objects changed instead of "Binary files differ". Branching and
merging drawings is the goal; this page says what works today and what comes next.

## Set up a repository

Tell git to read `.dwg` and `.dxf` files through CADCraft. In the repository's `.gitattributes`
(committed, so everyone gets it):

```gitattributes
*.dwg diff=cadcraft
*.dxf diff=cadcraft
```

And once per machine (`cadcraft-cli` on the `PATH`):

```sh
git config --global diff.cadcraft.textconv "cadcraft-cli textconv"
git config --global diff.cadcraft.cachetextconv true      # big drawings: convert each version once
git config --global difftool.cadcraft.cmd 'cadcraft-cli diff "$LOCAL" "$REMOTE"'
```

## Look at changes

`git diff`, `git log -p` and `git show` now list the drawing one line per object:

```diff
 103 Arc layer="0" {"center":{"x":6000.0,"y":1000.0,"z":0.0},"end":3.141593,"radius":900.0,…}
-105 Circle layer="0" {"center":{"x":9000.0,"y":6500.0,"z":0.0},"radius":400.0,"type":"circle"}
+105 Circle layer="0" {"center":{"x":7500.0,"y":5500.0,"z":0.0},"radius":400.0,"type":"circle"}
+109 Polyline layer="0" {"closed":true,…,"vertices":[…]}
```

Each line is an object's handle, type, layer, its properties where they differ from the defaults
(ByLayer colour and so on), and its geometry. Tables (layers, styles), block definitions and
layouts have their own sections. `git diff --word-diff` highlights the changed values.

For a review summary, `git difftool -t cadcraft main my-branch -- plan.dwg` runs COMPARE:

```text
Compared with plan.dwg: 3 only in this drawing, 0 only in plan.dwg, 3 changed, 6 the same.
+ Polyline 109
~ Circle 105 (geometry)
~ Text 108 (text)
```

## Compare with a branch in the app

Open the drawing from its git repository and choose **Tools → Compare with Branch…** (or type
`compare.branch`). The command line offers the repository's branches, `main` first, and `HEAD`
(the last commit, to see what you changed since): click one, or press Enter for the default. The
canvas shows the differences against that version of the same file: green only here, red only
there, unchanged objects dimmed; ◀ ▶ step through the areas with changes, ✕ ends it.

COMPARE takes a branch where it takes a file: `compare` then `main`, `HEAD~3`, a tag or a commit
id. Scripts and agents: `{"command":"compare","params":{"revision":"main"}}`, and
`compare.branches` lists what there is to compare with. The app runs the system's `git` to read
the old version (desktop only; save the drawing in the repository first).

The same listing is the `drawing.listing` command (control channel, MCP, scripts), so an agent
can read a drawing's state as text.

## What the listing guarantees

- The same drawing gives the same text, byte for byte: tables and blocks sorted by name,
  objects by handle, coordinates rounded to 1e-6.
- An edit changes only the lines of the objects it touched.
- Saving again changes nothing: what changes on every save is left out: header variables
  (timestamps, the current view), the generated geometry of dimensions (`*D` blocks), the numbers
  of anonymous blocks (`*U12` is listed by its contents as `*U#5f3a09c1`), the handles of the
  objects inside anonymous blocks, and the owner handle in objects CADCraft keeps as raw DXF.

**Known gap:** the first time CADCraft saves a drawing that came from AutoCAD, the diff can still
show a few changes the user did not make: an attribute on layer `0` is saved on its block
reference's layer, and tilted text, hatches and dimensions lose their plane (see
[gaps.md](gaps.md) FF15). On an 11 MB AutoCAD drawing that is 374 of 187,000 lines. Commit the
drawing once saved by CADCraft before branching to start from a clean diff.

## What comes next

1. ~~Readable `git diff` (`textconv`) and a COMPARE summary (`diff`)~~ (this page).
2. COMPARE as an image for pull requests: the overlay (green, red, grey) exported as PNG.
3. A git-native drawing format: a folder with one small text file per object, so git stores only
   the objects that changed and merges edits to different objects without conflicts. Needs
   deterministic output and handles that two branches can't both allocate.
4. A merge driver for edits to the same object, and a check after merging for references to
   deleted objects (dimensions, block references, groups).
