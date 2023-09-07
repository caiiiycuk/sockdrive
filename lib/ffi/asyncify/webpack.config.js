const path = require('path');
const { Compilation, sources } = require('webpack');
const ESLintPlugin = require('eslint-webpack-plugin');

module.exports = {
    mode: 'production',
    entry: './src/index.ts',
    module: {
        rules: [
            {
                test: /\.tsx?$/,
                use: 'ts-loader',
                exclude: /node_modules/,
            },
        ],
    },
    resolve: {
        extensions: ['.tsx', '.ts', '.js'],
    },
    output: {
        filename: 'bundle.js',
        path: path.resolve(__dirname, 'dist'),
    },
    plugins: [
        {
            apply(compiler) {
                compiler.hooks.thisCompilation.tap('Replace', (compilation) => {
                    compilation.hooks.processAssets.tap({ name: 'R_PLUGIN', stage: Compilation.PROCESS_ASSETS_STAGE_OPTIMIZE_HASH },
                        () => {
                            compilation.updateAsset('bundle.js',
                                new sources.RawSource("R\"'''(" + compilation.getAsset('bundle.js').source.source() + ")'''\""));
                        }
                    );
                });
            }
        },
        new ESLintPlugin({
            fix: true,
        }),
    ],
    optimization: {
        minimize: true,
    },
};