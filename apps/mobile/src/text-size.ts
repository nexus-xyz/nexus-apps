/**
 * The font scale from which a row stacks what sat side by side (a label and its value, a chat's title and its time),
 * as iOS's own lists do at the Larger Accessibility Sizes. On iOS AX1's body is 28 pt over the default 17 (a scale of
 * about 1.65); Extra Extra Extra Large, the largest standard size, is about 1.35. On Android it is the two largest font
 * sizes (1.8 and 2).
 */
export const ACCESSIBILITY_FONT_SCALE = 1.6;

/** Whether the person's text size is one of the accessibility sizes (React Native's `fontScale`). */
export const isAccessibilitySize = (fontScale: number) => fontScale >= ACCESSIBILITY_FONT_SCALE;

/**
 * The most a bar's or a chip's text grows (`maxFontSizeMultiplier`): they keep a fixed height, as iOS's own bars do,
 * and at the accessibility sizes their text would spill over the page or shrink to one letter.
 */
export const CHROME_FONT_SCALE = 1.3;
