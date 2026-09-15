import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const directory = new URL('.', import.meta.url);
const schema = JSON.parse(await readFile(new URL('immutable-record-log.schema.json', directory)));
const golden = JSON.parse(await readFile(new URL('immutable-record.golden.json', directory)));

const closed = (value, keys) => assert.deepEqual(Object.keys(value).sort(), [...keys].sort());

test('immutable record capability schema is closed to exactly append subscribe open', () => {
  closed(schema, ['$schema', '$id', 'title', 'type', 'additionalProperties', 'required', 'properties', '$defs']);
  assert.equal(schema.additionalProperties, false);
  assert.deepEqual(schema.properties.methods.properties && Object.keys(schema.properties.methods.properties).sort(), ['append', 'open', 'subscribe']);
  assert.deepEqual(schema.properties.methods.required, ['append', 'subscribe', 'open']);
  assert.equal(schema.$id, 'teamarness.synchronization.immutable-record-log/1.0');
});

test('golden fixture records implicit scope and bounded envelope encodings', () => {
  assert.equal(golden.version, 1);
  assert.equal(golden.append.canonicalHex.length, 18);
  assert.equal(golden.orderKey.encoding, 'version:u8 || causalHeight:u64 big-endian || recordId:32 bytes');
  assert.equal(golden.cursor.encoding, 'version:u8 || journalId:16 bytes || deliverySequence:u64 big-endian');
  assert.equal(golden.sealedEnvelope.cipherSuite, 1);
});

test('contract bounds are explicit even where decoded base64 size needs host validation', () => {
  const append = schema.$defs.append.properties.request.properties;
  assert.equal(append.purpose.$ref, '#/$defs/purpose');
  assert.equal(append.canonical.$ref, '#/$defs/base64url');
  assert.equal(schema.$defs.open.properties.request.properties.sealedRecords.maxItems, 32);
  assert.equal(schema.$defs.open.properties.request.properties.sealedRecords.minItems, 1);
  assert.equal(schema.$defs.stateEvent.properties.pendingRecordCount.pattern, '^(0|[1-9][0-9]*)$');
});
