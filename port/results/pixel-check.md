# Milestone 20 pixel baseline

`port/tools/pixel-check` on the Mac Studio, current branch `port-m20-sweep`; threshold 0, anti-aliased pixels counted apart. Counts are changed pixels out of 3,145,728, at the tool's 10% fuzz.

| window | dark | light | hc | hclight |
|---|---:|---:|---:|---:|
| empty | 3567 | 3600 | 398 | 401 |
| folder | 7174 | 4003 | 896 | 810 |
| file | 8694 | 5150 | 3060 | 3871 |
| main | 9501 | 5929 | 4940 | 4594 |
| multiroot | 7307 | 4079 | 1046 | 887 |
| folded | 12734 | 26177 | 5641 | 7656 |
| find | 40897 | 20313 | 7049 | 15248 |
| quickopen | 42402 | 62609 | 10541 | 20981 |
| split | 12678 | 9662 | 6109 | 10736 |
| panel | 29397 | 26061 | 30761 | 33056 |

## Remaining regions

- `main`, `folded`: minimap selection/word-highlight pixels and the overview-ruler/minimap boundary; the port now suppresses initial minimap selections and paints the ruler background over right-side minimap highlights. Recheck the next capture; any remaining 1 px differences need attribution to rasterization or region-specific cause.
- `split`: sash/separator 1 px alignment; port separator now follows `splitview.css`'s start-of-view line. Recheck the next capture.
- `quickopen`: row action positions and quick-input shadow/frosted fill; port now includes the upstream Open to the Side/Bottom action and adjusts spacing. Shadow blur and backdrop blur differ between GPUI and Chromium.
- `find`: status/minimap region, find widget shadow and text width; inspect remaining regions against the current diff images before calling parity.
- `panel`: panel title overflow action/ellipsis differs from the reference's additional-views affordance; not fixed.
- Other windows: pixel counts include glyph anti-aliasing, font rasterization and platform rendering differences; region-level inspection remains incomplete.
