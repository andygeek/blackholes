export type AppTheme = "light" | "dark";

export const normalizeTheme = (value?: string | null): AppTheme => (
  value === "light" ? "light" : "dark"
);

export const applyAppTheme = (value?: string | null): AppTheme => {
  const theme = normalizeTheme(value);
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
  return theme;
};
