/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        // Oh-Ben-Claw brand palette — deep ocean + electric cyan
        obc: {
          50:  "#e6f7ff",
          100: "#b3e8ff",
          200: "#80d9ff",
          300: "#4dcaff",
          400: "#1abbff",
          500: "#00aaee",  // primary
          600: "#0088cc",
          700: "#006699",
          800: "#004466",
          900: "#002233",
          950: "#001122",
        },
        surface: {
          DEFAULT: "#0f1923",
          raised: "#162130",
          overlay: "#1d2d3e",
          border: "#243547",
        },
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
        mono: ["JetBrains Mono", "Fira Code", "monospace"],
      },
      animation: {
        "pulse-slow": "pulse 3s cubic-bezier(0.4, 0, 0.6, 1) infinite",
        "fade-in": "fadeIn 0.2s ease-in-out",
        "slide-up": "slideUp 0.2s ease-out",
      },
      keyframes: {
        fadeIn: {
          "0%": { opacity: "0" },
          "100%": { opacity: "1" },
        },
        slideUp: {
          "0%": { transform: "translateY(8px)", opacity: "0" },
          "100%": { transform: "translateY(0)", opacity: "1" },
        },
      },
    },
  },
  plugins: [],
};
