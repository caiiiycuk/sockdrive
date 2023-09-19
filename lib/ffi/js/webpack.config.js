const path = require("path");
const { Compilation, sources, ProvidePlugin } = require("webpack");
const ESLintPlugin = require("eslint-webpack-plugin");

module.exports = {
    devtool: "source-map",
    mode: "production",
    entry: {
        sockdrive: "./src/index.ts",
        test: "./src/test/index.ts",
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
            buffer: require.resolve("buffer")
        }
    },
    output: {
        filename: "[name].js",
        path: path.resolve(__dirname, "dist"),
    },
    plugins: [
        new ProvidePlugin({
            process: "process/browser",
            Buffer: ["buffer", "Buffer"],
        }),
        new ESLintPlugin({
            fix: true,
            extensions: ["ts"],
        }),
    ],
    optimization: {
        minimize: true,
    },
};