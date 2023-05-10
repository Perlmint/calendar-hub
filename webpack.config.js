const HtmlWebpackPlugin = require("html-webpack-plugin");

const production = process.env.PRODUCTION === "1";

/** @type {import('webpack').Configuration} */
const configuration = {
    mode: (production ? "production" : "development"),
    entry: "./src/index.tsx",
    module: {
        rules: [
            {
                test: /\.tsx?$/,
                use: 'ts-loader',
                exclude: /node_modules/,
            },
            {
                test: /\.css$/i,
                use: [
                    "style-loader",
                    "css-loader",
                ],
            },
        ],
    },
    resolve: {
        extensions: ['.tsx', '.ts', '.js'],
    },
    plugins: [
        new HtmlWebpackPlugin({
            template: "./src/index.html"
        }),
    ],
    devtool: production ? "hidden-source-map" : "eval-cheap-module-source-map",
};

module.exports = configuration;
