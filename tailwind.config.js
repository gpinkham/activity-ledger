/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        bg: "var(--bg)",
        surface: "var(--surface)",
        elevated: "var(--surface-2)",
        line: "var(--border)",
        ink: "var(--text)",
        muted: "var(--muted)",
        accent: "var(--c1)",
      },
      boxShadow: {
        glow: "0 0 28px -8px var(--glow)",
        "glow-strong": "0 0 40px -6px var(--glow)",
      },
    },
  },
  plugins: [],
};
