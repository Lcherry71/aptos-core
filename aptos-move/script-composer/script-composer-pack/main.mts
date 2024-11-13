// @ts-ignore
import * as wasmModule from '../pkg/aptos_dynamic_transaction_composer_bg.wasm';
export * from "../pkg/aptos_dynamic_transaction_composer.js";
const wasm = new WebAssembly.Module(new Uint8Array(wasmModule.default));
export { wasm };