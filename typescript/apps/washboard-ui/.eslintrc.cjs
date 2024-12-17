const path = require('node:path');

const config = {
  extends: ['@wasmcloud/eslint-config'],
  parserOptions: {
    tsconfigRootDir: __dirname,
    project: ['./tsconfig.eslint.json', './tsconfig.json'],
  },
  settings: {
    tailwindcss: {
      config: path.resolve(__dirname, './tailwind.config.ts'),
      callees: ['classnames', 'clsx', 'ctl', 'cn', 'cva'],
    },
    'import/resolver': {
      typescript: {
        alwaysTryTypes: true,
        project: [
          path.resolve(__dirname, './tsconfig.json'),
          path.resolve(__dirname, './tsconfig.eslint.json'),
        ],
      },
    },
  },
  overrides: [
    {
      files: ['*.spec.ts?(x)', '*.fixture.ts?(x)', '*.test.ts?(x)'],
      rules: {
        // the 'use' function is not a react hook
        'react-hooks/rules-of-hooks': 'off',
        // playwright Locators do not have the dataset property
        'unicorn/prefer-dom-node-dataset': 'off',
      },
    },
  ],
};

module.exports = config;
