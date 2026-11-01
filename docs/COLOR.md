# 🎨 Galerith's perceptual project identity

- **Theme:** archival jade green, distinct from Posthorn amber, Ferrua teal, Typefinch periwinkle and Rust Age iron oxide.
- **sRGB:** `#90D8AA`
- **OKLCH:** `oklch(82.22% 0.0973 155.6)`
- **OKLab:** `L=0.82221, a=-0.08856, b=0.04019`
- **Swatch:** [assets/galerith-color.svg](../assets/galerith-color.svg)
- **Background:** deep green-charcoal `#19251F`; use jade as an accent, not the body-text color.

Color was evaluated in perceptually uniform OKLab/OKLCH, rather than assigning arbitrary RGB values. Interface contrasts still require separate acceptance testing.

## Contrast check

Measured the exact foreground/background pairs used in `assets/galerith-color.svg` with the WCAG relative-luminance contrast formula:

- Jade `#90D8AA` on charcoal `#19251F`: **9.50:1**. This covers the 18px jade label and the jade swatch against its background.
- Off-white `#F7FAF8` on charcoal `#19251F`: **15.07:1** for the Galerith wordmark.
- Charcoal `#19251F` on jade `#90D8AA`: **9.50:1** for the mountain and circle details.

The text pairs exceed WCAG 2.2 AA's 4.5:1 minimum for normal text; the meaningful icon details exceed the 3:1 non-text contrast minimum. See [Contrast (Minimum), SC 1.4.3](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum) and [Non-text Contrast, SC 1.4.11](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast). This verifies the repository's current brand asset and foreground/background pair, not arbitrary themes or future interface surfaces.
