const path = require('node:path');

const config = {
  extends: ['@wasmcloud/eslint-config'],
  parserOptions: {
    tsconfigRootDir: __dirname,
    project: [
      './tsconfig.eslint.json',
      './tsconfig.json',
    ],
  },
  settings: {
  'import/resolver': {
    typescript: {
      alwaysTryTypes: true,
      project: [
        path.resolve(__dirname, './tsconfig.json'),
        path.resolve(__dirname, './tsconfig.eslint.json'),
      ],
    },
  },
}
};

module.exports = config;
