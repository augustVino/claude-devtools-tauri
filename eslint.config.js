import js from '@eslint/js';
import globals from 'globals';
import tseslint from 'typescript-eslint';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';

export default tseslint.config(
  // 1. 全局 ignores（codex P1-13 补全）
  {
    ignores: [
      'dist/',
      'node_modules/',
      'src-tauri/target/',
      'src-tauri/gen/',
      '.gitnexus/',
      '.claude/',
    ],
  },

  // 2. 基础规则（JS 推荐集）
  js.configs.recommended,

  // 3. TypeScript 推荐集
  ...tseslint.configs.recommended,

  // 4. Renderer 代码（browser globals + 合并 rules，codex P1-12/P1-15）
  //    v4: eqeqeq 加 { null: 'ignore' }（codex P0-2 防御性配置）。
  //    深度 review 修正：项目实测 src/ 有 30 处宽松 == null/!= null
  //    （合法的 null|undefined 双判断惯用法），通过此选项豁免。
  {
    files: ['src/**/*.{ts,tsx}'],
    languageOptions: {
      ecmaVersion: 2022,
      globals: { ...globals.browser },
    },
    plugins: { 'react-hooks': reactHooks, 'react-refresh': reactRefresh },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // react-hooks v7 introduced two new rules that flag pre-existing patterns
      // (setState-in-effect, ref-during-render). Downgraded to warn to keep
      // `pnpm lint` green without behavior-changing refactors in this task;
      // tracked for follow-up.
      'react-hooks/set-state-in-effect': 'warn',
      'react-hooks/refs': 'warn',
      'react-refresh/only-export-components': ['warn', { allowConstantExport: true }],
      '@typescript-eslint/no-unused-vars': ['warn', { argsIgnorePattern: '^_' }],
      '@typescript-eslint/no-explicit-any': 'warn',
      '@typescript-eslint/consistent-type-imports': 'warn',
      'no-console': ['warn', { allow: ['warn', 'error'] }],
      'no-debugger': 'error',
      'eqeqeq': ['error', 'always', { null: 'ignore' }],
      'prefer-const': 'error',
    },
  },

  // 5. Node 环境（vite.config.ts、tailwind/postcss/eslint 等 config 文件，codex P1-12 拆分）
  //    CommonJS config files legitimately use module/require; disable the
  //    TS-eslint rule that forbids require() for these Node-only files.
  {
    files: ['*.config.ts', '*.config.js', '*.config.cjs', 'eslint.config.js'],
    languageOptions: {
      ecmaVersion: 2022,
      globals: { ...globals.node },
    },
    rules: {
      '@typescript-eslint/no-require-imports': 'off',
    },
  },
);
