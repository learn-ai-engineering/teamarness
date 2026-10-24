import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const directory = new URL('.', import.meta.url);
const schema = JSON.parse(await readFile(new URL('immutable-record-log.schema.json', directory)));
const golden = JSON.parse(await readFile(new URL('immutable-record.golden.json', directory)));
const schema11 = JSON.parse(await readFile(new URL('immutable-record-log-1.1.schema.json', directory)));
const replay = JSON.parse(await readFile(new URL('immutable-record-replay.golden.json', directory)));

const closed = (value, keys) => assert.deepEqual(Object.keys(value).sort(), [...keys].sort());

test('immutable record capability schema is closed to exactly append subscribe open', () => {
  closed(schema, ['$schema', '$id', 'title', 'type', 'additionalProperties', 'required', 'properties', '$defs']);
  assert.equal(schema.additionalProperties, false);
  assert.deepEqual(schema.properties.methods.properties && Object.keys(schema.properties.methods.properties).sort(), ['append', 'open', 'subscribe']);
  assert.deepEqual(schema.properties.methods.required, ['append', 'subscribe', 'open']);
  assert.equal(schema.$id, 'teamarness.synchronization.immutable-record-log/1.1');
});

test('v1.1 adds only a closed credited replay-end event while v1.0 remains unchanged', () => {
  assert.equal(schema.$defs.subscribe.properties.events.items.oneOf.length, 3);
  assert.equal(schema11.$id, 'teamarness.synchronization.immutable-record-log/1.1');
  assert.deepEqual(schema11.$defs.subscribe.properties.events.items.oneOf.slice(0, 3), schema.$defs.subscribe.properties.events.items.oneOf);
  assert.deepEqual(schema11.$defs.subscribe.properties.events.items.oneOf[3], { $ref: '#/$defs/replayEndEvent' });
  closed(schema11.$defs.replayEndEvent.properties, ['kind', 'cursor', 'gapFree']);
  assert.equal(schema11.$defs.replayEndEvent.additionalProperties, false);
  assert.equal(replay.kind, 'replay_end');
  assert.match(replay.cursor, new RegExp(schema11.$defs.cursor.pattern));
  assert.equal(replay.gapFree, true);
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
  assert.equal(append.principal.$ref, '#/$defs/principalId');
  assert.equal(schema.$defs.principalId.pattern, '^[0-9a-f]{64}$');
  assert.equal(schema.$defs.open.properties.request.properties.sealedRecords.maxItems, 32);
  assert.equal(schema.$defs.open.properties.request.properties.sealedRecords.minItems, 1);
  assert.equal(schema.$defs.stateEvent.properties.pendingRecordCount.pattern, '^(0|[1-9][0-9]*)$');
});

test('state and warning events are closed and stable', () => {
  const state = schema.$defs.stateEvent;
  closed(state.properties, ['kind', 'availability', 'pendingRecordCount']);
  assert.equal(state.additionalProperties, false);
  assert.deepEqual(state.properties.availability.enum, ['waiting_for_history_access', 'offline', 'catching_up', 'current']);
  assert.equal(state.properties.availability.enum.includes('degraded'), false);

  const warning = schema.$defs.warningEvent;
  closed(warning.properties, ['kind', 'code']);
  assert.equal(warning.additionalProperties, false);
  assert.equal(warning.properties.code.pattern, '^[a-z][a-z0-9_]{0,63}$');
  for (const code of ['record_quarantined', 'scope_quarantined', 'membership_revoked', 'principal_not_bound']) {
    assert.match(code, new RegExp(warning.properties.code.pattern));
  }
});

test('append request 1.1 adds principal as an optional selector and keeps 1.0 requests valid', () => {
  const request = schema.$defs.append.properties.request;
  assert.equal(request.additionalProperties, false);
  closed(request.properties, ['purpose', 'canonical', 'principal']);
  assert.deepEqual(request.required, ['purpose', 'canonical']);
  assert.equal(request.required.includes('principal'), false);
});
