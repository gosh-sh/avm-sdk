// Guest ABI execution only, not host admission or candidate runtime validation.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
const [wasmPath, vectorsPath, evidencePath] = process.argv.slice(2);
const wasm = fs.readFileSync(wasmPath);
const module = await WebAssembly.compile(wasm);
const cases = [];
for (const row of fs.readFileSync(vectorsPath, 'utf8').trim().split('\n')) {
  const [name, entrypoint, inputHex, expectedHex] = row.split('\t');
  const instance = await WebAssembly.instantiate(module, {});
  const guest = instance.exports;
  const input = Buffer.from(inputHex, 'hex');
  const ptr = guest.avm_guest_alloc(input.length);
  assert.ok(ptr, `${name}: input allocation`);
  new Uint8Array(guest.memory.buffer, ptr, input.length).set(input);
  const packed = BigInt.asUintN(64, guest[entrypoint](ptr, input.length));
  assert.notEqual(packed, 0n, `${name}: guest returned missing output (mode mismatch?)`);
  const outputPtr = Number(packed >> 32n);
  const outputLen = Number(packed & 0xffffffffn);
  const output = Buffer.from(new Uint8Array(guest.memory.buffer, outputPtr, outputLen));
  assert.equal(output.toString('hex'), expectedHex, `${name}: WASM/native canonical output`);
  assert.deepEqual(Buffer.from(new Uint8Array(guest.memory.buffer, ptr, input.length)), input,
                   `${name}: input ownership`);
  guest.avm_guest_free(outputPtr, outputLen);
  guest.avm_guest_free(ptr, input.length);
  cases.push({ name, entrypoint, input_bytes: input.length, output_bytes: outputLen });
}
fs.writeFileSync(evidencePath, JSON.stringify({ status: 'PASS',
  scope: 'Guest ABI only; published guest SDK 0.1.1, Node WebAssembly engine',
  wasm_sha256: crypto.createHash('sha256').update(wasm).digest('hex'), cases }, null, 2) + '\n');
console.log(`Audit WASM/native output agreement and rejected-mode serialization: ${cases.length} PASS`);
