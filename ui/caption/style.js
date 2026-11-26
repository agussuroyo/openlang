// Turns the saved subtitle style into CSS. A new style field only needs one line here.
const variables = {
  text_size: size => ['--sub-size', `${size}px`],
  color: color => ['--sub-color', color],
  font: font => ['--sub-font', font && `"${font}"`],
};

export function applyStyle(style) {
  const root = document.documentElement.style;
  for (const [field, toCss] of Object.entries(variables)) {
    const [name, value] = toCss(style[field]);
    if (value) root.setProperty(name, value);
    else root.removeProperty(name);
  }
  document.body.dataset.show = style.show;
}
