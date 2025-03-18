import resolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import commonjs from "@rollup/plugin-commonjs";
import eslint from "@rollup/plugin-eslint";
// import dts from "rollup-plugin-dts";

export default [
  {
    input: "src/main.ts",
    output: [
      {
        file: "dist/sockdrive.cjs.js",
        format: "cjs",
        sourcemap: true,
      },
      {
        file: "dist/sockdrive.esm.js",
        format: "esm",
        sourcemap: true,
      },
      {
        file: "dist/sockdrive.umd.js",
        format: "umd",
        sourcemap: true,
        name: "sockdrive",
      },
    ],
    plugins: [
      eslint({
        fix: true,
      }),
      resolve(),
      commonjs(),
      typescript({ tsconfig: "./tsconfig.json" }),
    ],
  },
  // {
  //   input: "dist/esm/types/main.d.ts",
  //   output: [{ file: "dist/sockdrive.d.ts", format: "esm" }],
  //   plugins: [dts()],
  // },
];
