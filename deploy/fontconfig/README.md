# Texly font fallback / safety net

Goal: a missing font should **degrade visually**, never kill the compile.

## What's in here

| File | Installed to | Purpose |
|------|--------------|---------|
| `99-texly.conf` | `/etc/fonts/conf.d/99-texly.conf` | fontconfig alias families `__texly_serif` / `__texly_sans` / `__texly_mono` with a prefer-chain over the bundled base set |
| `texly-fonts.tex` | `/usr/share/texly/texly-fonts.tex` | `\input`-able preamble helpers that degrade gracefully |

## Validated finding (important)

The original plan was to rely on the fontconfig aliases so that
`\setmainfont{__texly_serif}` would resolve to a bundled font. **This does not
work with Tectonic.**

- `fc-match __texly_serif` → `TeX Gyre Termes` ✓ (system fontconfig honors the alias)
- `\setmainfont{__texly_serif}` inside Tectonic → **"font cannot be found"** ✗

Tectonic's XeTeX font lookup scans the system font *files* but matches by real
family name; it ignores fontconfig `<alias>` rules. The `99-texly.conf` is kept
because it satisfies `fc-match` and would help any non-Tectonic consumer, but it
is **not** the effective safety net for compiles.

## The effective safety net: `texly-fonts.tex`

fontspec's `\IfFontExistsTF` *does* work in Tectonic, so the helpers try the
requested family and fall back to a **real bundled family** if it is absent:

```latex
\documentclass{article}
\input{/usr/share/texly/texly-fonts}   % provides \texlySetMain/Sans/Mono
\texlySetMain{EB Garamond 12}          % degrades to TeX Gyre Termes / Latin Modern if missing
\texlySetSans{Fira Sans}
\texlySetMono{JetBrains Mono}
\begin{document}
...
\end{document}
```

Fallback chains:

- serif → `TeX Gyre Termes` → `Latin Modern Roman`
- sans  → `TeX Gyre Heros`  → `Latin Modern Sans`
- mono  → `TeX Gyre Cursor` → `Latin Modern Mono`

## Open question — preamble injection

Texly does **not** generate or inject any preamble: `compile.rs` runs
`tectonic <file>` on the user's document as-is. So `\IfFontExistsTF` / the
helpers must live in the **user document** (via `\input{/usr/share/texly/texly-fonts}`).

If automatic protection is wanted later, a preamble-preprocessor in the Texly
compile pipeline could `\input` this file for every compile — that is a separate
feature (related to the dynamic font-downloader ticket) and intentionally out of
scope here.

## Note on EB Garamond

Debian's `fonts-ebgaramond` is the optical-size variant; bare
`\setmainfont{EB Garamond}` fails (non-standard style names). Use
`EB Garamond 12`. See the Dockerfile font comment.
