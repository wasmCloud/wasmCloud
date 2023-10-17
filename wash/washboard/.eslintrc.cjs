const {resolve, join} = require('node:path');

module.exports = {
  root: true,
  env: {browser: true, es2020: true},
  extends: [
    'eslint:recommended',
    'plugin:import/recommended',
    'plugin:import/typescript',
    'plugin:@typescript-eslint/recommended',
    'plugin:tailwindcss/recommended',
    'plugin:eslint-comments/recommended',
    'plugin:react/recommended',
    'plugin:react-hooks/recommended',
    'plugin:unicorn/recommended',
    'plugin:prettier/recommended',
  ],
  parser: '@typescript-eslint/parser',
  parserOptions: {
    ecmaVersion: 'latest',
    sourceType: 'module',
    tsconfigRootDir: __dirname,
    project: ['./tsconfig.json', './tsconfig.eslint.json'],
  },
  plugins: ['react-refresh', 'prettier'],
  rules: {
    'object-curly-spacing': ['warn', 'never'],
    'no-console': ['warn', {allow: ['info', 'warn', 'error']}],
    'no-undef': 'warn',
    'no-unreachable': 'warn',
    'no-param-reassign': 'warn',
    'no-case-declarations': 'warn',
    'no-unneeded-ternary': 'warn',
    'spaced-comment': ['warn', 'always', {markers: ['/']}],
    'react-refresh/only-export-components': 'warn',
    'react/react-in-jsx-scope': 'off',
    'react/prop-types': 'off',
    'eslint-comments/disable-enable-pair': ['error', {allowWholeFile: true}],
    'eslint-comments/require-description': 'warn',
    'eslint-comments/no-unused-disable': 'error',
    '@typescript-eslint/no-unused-vars': ['warn', {ignoreRestSiblings: true}],
    '@typescript-eslint/no-loss-of-precision': 'warn',
    '@typescript-eslint/explicit-function-return-type': [
      'warn',
      {
        allowExpressions: true,
        allowTypedFunctionExpressions: true,
        allowHigherOrderFunctions: false,
      },
    ],
    '@typescript-eslint/explicit-member-accessibility': 'warn',
    '@typescript-eslint/member-ordering': [
      'warn',
      {
        default: 'never',
        classes: ['field', 'constructor', 'method'],
      },
    ],
    'import/no-cycle': 'error',
    'import/no-named-as-default-member': 'off',
    'import/order': [
      'warn',
      {
        alphabetize: {
          order: 'asc',
        },
        groups: [
          'unknown',
          'type',
          'builtin',
          'external',
          'internal',
          'object',
          'parent',
          'sibling',
          'index',
        ],
        warnOnUnassignedImports: true,
        pathGroups: [
          {
            group: 'unknown',
            pattern: '**/*.+(css|sass|scss|less|styl)',
            patternOptions: {dot: true, nocomment: true},
            position: 'before',
          },
          {
            group: 'unknown',
            pattern: '{.,..}/**/*.+(css|sass|scss|less|styl)',
            patternOptions: {dot: true, nocomment: true},
            position: 'before',
          },
        ],
      },
    ],
    'unicorn/prevent-abbreviations': [
      'error',
      {
        replacements: {
          prop: false,
          props: false,
          ref: false,
          refs: false,
        },
      },
    ],
    'unicorn/no-array-reduce': 'off',
    'unicorn/no-null': 'off',
  },
  overrides: [
    {
      files: ['*.jsx', '*.tsx'],
      rules: {
        'unicorn/filename-case': ['error', {case: 'pascalCase'}],
      },
    },
    {
      files: [
        '*rc.cjs',
        '*rc.ts',
        '*rc.js',
        '*.config.js',
        '*.config.cjs',
        '*.config.ts',
        'vite.config.ts',
      ],
      env: {
        node: true,
      },
      rules: {
        '@typescript-eslint/no-var-requires': 'off',
      },
    },
  ],
  settings: {
    'import/parsers': {
      '@typescript-eslint/parser': ['.ts', '.tsx'],
    },
    'import/resolver': {
      typescript: {
        alwaysTryTypes: true,
        project: [
          resolve(__dirname, './tsconfig.json'),
          resolve(__dirname, './tsconfig.eslint.json'),
        ],
      },
      tailwindcss: {
        callees: ['classnames', 'clsx', 'ctl', 'cn', 'cva'],
      },
    },
    react: {
      version: 'detect',
    },
  },
  ignorePatterns: ['node_modules', 'dist', 'build', 'coverage'],
};
