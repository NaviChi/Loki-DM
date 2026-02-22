/** @type {import('tailwindcss').Config} */
export default {
  darkMode: "class",
  content: ["./index.html", "./src/**/*.{js,jsx,ts,tsx}"],
  theme: {
    extend: {
      boxShadow: {
        neon: "0 0 0 1px rgba(73,162,255,0.45), 0 0 18px rgba(73,162,255,0.28)",
      },
      keyframes: {
        pulseGlow: {
          "0%, 100%": { boxShadow: "0 0 0 0 rgba(73,162,255,0.15)" },
          "50%": { boxShadow: "0 0 0 6px rgba(73,162,255,0.0)" },
        },
      },
      animation: {
        pulseGlow: "pulseGlow 2.2s ease-in-out infinite",
      },
    },
  },
  plugins: [],
};
