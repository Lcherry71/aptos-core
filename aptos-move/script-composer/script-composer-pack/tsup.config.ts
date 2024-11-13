import { defineConfig } from 'tsup'
import { Plugin } from 'esbuild'
import fs from 'fs'
import path from 'path'

export function wasmLoader():Plugin {
  return {
    name: 'wasm-loader',
    setup(build) {
      build.onLoad({ filter: /\.wasm$/ }, async (args) => {
        const wasmPath = path.resolve(args.path);
        const wasmBuffer = await fs.promises.readFile(wasmPath);
        const contents = `
          const wasm = [${new Uint8Array(wasmBuffer).toString()}];
          export default wasm;
        `;
        return {
          contents,
          loader: 'js',
        };
      });
    },
  };
}
export default defineConfig({
  entry: ['main.mts'],
  splitting: false,
  clean: true,
  dts: {
    entry: 'main.mts',
    resolve: true,
  },
  format: ['cjs', 'esm'],
  esbuildPlugins: [wasmLoader()],
  esbuildOptions(options, context) {
    if (context.format === 'cjs') {
      options.outdir = 'dist/cjs'
    } else if (context.format === 'esm') {
      options.outdir = 'dist/esm'
    }
  }
})