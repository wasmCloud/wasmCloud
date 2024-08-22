/** @type {import('eslint').Linter.Config} */
const config = {
  extends: ['@wasmcloud/eslint-config'],
  ignorePatterns: ['dist', 'node_modules'],
  env: {
    node: true,
  },
  overrides: [
    {
      files: ['*.js', '*.cjs'],
      rules: {
        '@typescript-eslint/no-require-imports': 'off',
        '@typescript-eslint/no-var-requires': 'off',
        '@typescript-eslint/naming-convention': 'off',
        'import/no-default-export': 'off',
        'unicorn/prefer-module': 'off',
      },
    },
  ],
};

module.exports = config;
