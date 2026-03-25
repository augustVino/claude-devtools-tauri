/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    './index.html',
    './src/**/*.{js,ts,jsx,tsx}',
  ],
  theme: {
    extend: {
      colors: {
        // Theme-aware surface colors (use CSS variables)
        surface: {
          DEFAULT: 'var(--color-surface)',
          raised: 'var(--color-surface-raised)',
          overlay: 'var(--color-surface-overlay)',
          sidebar: 'var(--color-surface-sidebar)',
          code: 'var(--code-bg)',
        },
        // Theme-aware border colors (use CSS variables)
        border: {
          DEFAULT: 'var(--color-border)',
          subtle: 'var(--color-border-subtle)',
          emphasis: 'var(--color-border-emphasis)',
        },
        // Theme-aware text colors (use CSS variables)
        text: {
          DEFAULT: 'var(--color-text)',
          secondary: 'var(--color-text-secondary)',
          muted: 'var(--color-text-muted)',
        },
        // Semantic colors (only for status, not containers)
        semantic: {
          success: '#22c55e',
          error: '#ef4444',
          warning: '#f59e0b',
          info: '#3b82f6',
        },
        // Theme-aware colors using CSS variables
        'claude-dark': {
          bg: 'var(--color-surface)',
          surface: 'var(--color-surface-raised)',
          border: 'var(--color-border)',
          text: 'var(--color-text)',
          'text-secondary': 'var(--color-text-secondary)'
        }
      }
    }
  },
  plugins: [
    require('@tailwindcss/typography')
  ]
}
