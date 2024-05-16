const path = require("path");
const { Compilation, sources, ProvidePlugin } = require("webpack");
const ESLintPlugin = require("eslint-webpack-plugin");

module.exports = {
    devtool: "source-map",
    mode: "production",
    entry: {
        sockdriveFat: "./src/sockdrive-fat.ts",
        sockdriveNative: "./src/sockdrive-native.ts",
        test: "./src/test/test.ts",
    },
    module: {
        rules: [
            {
                test: /\.tsx?$/,
                use: "ts-loader",
                exclude: /node_modules/,
            },
        ],
    },
    resolve: {
        extensions: [".tsx", ".ts", ".js"],
        fallback: {
            stream: require.resolve("stream-browserify"),
            buffer: require.resolve("buffer"),
        },
    },
    output: {
        filename: "[name].js",
        path: path.resolve(__dirname, "dist"),
    },
    plugins: [
        {
            apply(compiler) {
                compiler.hooks.thisCompilation.tap("Replace", (compilation) => {
                    compilation.hooks.processAssets.tap(
                        {
                            name: "R_PLUGIN",
                            stage: Compilation.PROCESS_ASSETS_STAGE_OPTIMIZE_HASH,
                        },
                        () => {
                            compilation.updateAsset("sockdriveNative.js",
                                new sources.RawSource(
                                    "R\"'''(" +
                                    compilation.getAsset("sockdriveNative.js").source.source() +
                                    ")'''\""),
                            );
                        },
                    );
                });
            },
        },
        new ProvidePlugin({
            process: "process/browser",
            Buffer: ["buffer", "Buffer"],
        }),
        new ESLintPlugin({
            fix: true,
            extensions: ["ts"],
            useEslintrc: false,
            overrideConfigFile: ".eslintrc.json",
        }),
    ],
    optimization: {
        minimize: true,
    },
};
