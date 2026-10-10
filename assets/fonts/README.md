# Bundled fonts

lawh never uses system fonts. These files are embedded into the binary with `include_bytes!`
(see `crates/lawh-math/src/fonts.rs`), so text and math look the same on every platform.

| File | Font | Version | Source | Licence | SHA-256 |
|---|---|---|---|---|---|
| `Inter-Regular.ttf` | Inter Regular (static) | 4.1 | `extras/ttf/Inter-Regular.ttf` from [Inter-4.1.zip](https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip) ([release](https://github.com/rsms/inter/releases/tag/v4.1)) | OFL-1.1 (`Inter-OFL.txt`) | `40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82` |
| `LibertinusMath-Regular.otf` | Libertinus Math Regular | 7.051 | `Libertinus-7.051/static/OTF/LibertinusMath-Regular.otf` from [Libertinus-7.051.zip](https://github.com/alerque/libertinus/releases/download/v7.051/Libertinus-7.051.zip) ([release](https://github.com/alerque/libertinus/releases/tag/v7.051)) | OFL-1.1 (`Libertinus-OFL.txt`) | `ded714b0d3808af527438ed7a85f16933133f66a12104c4a83db4cc248888011` |

The licence files are copied unchanged from each release archive (`LICENSE.txt` in the Inter
archive, `OFL.txt` in the Libertinus archive).

Archive checksums (SHA-256), for re-downloading:

- `Inter-4.1.zip`: `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`
- `Libertinus-7.051.zip`: `4d9be29b5cb380c35af8ba967abcc752ad1e07be1f738a9789c33e0dd7478c92`

Inter is the text font; Libertinus Math is the math font (it has an OpenType `MATH` table, which
Typst needs for math layout). Keep the static Inter, not the variable font, so Typst and the renderer
see the same outlines without variation axes.
