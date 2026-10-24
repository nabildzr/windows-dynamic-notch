# Vendored crates

## vello_encoding 0.11.0

Unmodified copy of the crates.io release except for the hand-picked
"bump allocated" buffer sizes in `src/config.rs` (`BufferSizes::new`).

Upstream sizes them for very large scenes (paris-30k), which allocates
about 165 MB of GPU buffers on the first frame. On integrated GPUs that
memory is carved out of system RAM and shows up in the process' private
working set. The notch draws at most a few hundred paths on a canvas of at
most 1400x520 physical pixels, so the sizes were reduced:

| buffer       | upstream | here     |
|--------------|----------|----------|
| bin_data     | 1 << 18  | 1 << 17  |
| tiles        | 1 << 21  | 1 << 17  |
| lines        | 1 << 21  | 1 << 17  |
| seg_counts   | 1 << 21  | 1 << 18  |
| segments     | 1 << 21  | 1 << 18  |
| blend_spill  | 1 << 20  | 1 << 16  |
| ptcl         | 1 << 23  | 1 << 20  |

Licensed under Apache-2.0 OR MIT (see the license files in the folder).
