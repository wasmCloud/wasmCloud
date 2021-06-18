// tailwind.config.js
//
// documentation on purge options:
//    https://tailwindcss.com/docs/optimizing-for-production#removing-unused-css
//
module.exports = {
  purge: [
      "../templates/*.hbs"
  ],
  darkMode: false, // or 'media' or 'class'
  theme: {
    fontFamily: {
      sans: ['Georgia','sans-serif'],
      serif: ['serif'],
    },
    extend: {
      typography: {
        DEFAULT: {
          css: {
            /* table of contents */
            '#toc div' : {
              paddingLeft: '1.5em',
            },
            '#toc a' : {
              color: "rgba(107, 114, 128)", // gray-500
              textDecoration: 'none',
            },
            '#toc p' : {
              color: "rgba(55, 65, 81)", // gray-700
              marginTop: 0,
              marginBottom: 0,
            },

            /* version table */
            '#version_block table td' : {
                paddingBottom: 0,
                paddingTop: 0,
            },
            // Slightly smaller headings
            h1: { fontSize: "2em" },
            h2: { fontSize: "1.6em" },
            h3: { fontSize: "1.3em" },
            h4: { fontSize: "1.1em" },
          }
        }
      }
    },
  },
  variants: {
    extend: {},
  },
  plugins: [
      require('@tailwindcss/typography')
  ],
}
