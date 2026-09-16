/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        sky: { DEFAULT: '#38BDF8', strong: '#0EA5E9', dark: '#0284C7' },
        ink: '#07111F',
        surface: '#0B1726',
      },
    },
  },
  plugins: [],
}
