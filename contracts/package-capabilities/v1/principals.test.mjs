import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const directory = new URL('.', import.meta.url);
const schema = JSON.parse(await readFile(new URL('principals.schema.json', directory)));

const closed = (value, keys) => assert.deepEqual(Object.keys(value).sort(), [...keys].sort());

test('principals capability schema is closed and presentation-only', () => {
  closed(schema, ['$schema', '$id', 'title', 'description', '$defs']);
  assert.match(schema.title, /^Workspace Principals capability 1\.0$/);
  closed(schema.$defs, ['cursor', 'workspaceId', 'identity', 'principalId', 'principal', 'snapshot', 'currentCall', 'lookupCall', 'lookupResult', 'subscribe']);
});

test('a principal row carries kind, steward, current keys, name and fingerprint', () => {
  const principal = schema.$defs.principal;
  assert.equal(principal.additionalProperties, false);
  closed(principal.properties, ['principalId', 'kind', 'stewardPrincipalId', 'keys', 'name', 'fingerprint']);
  assert.deepEqual(principal.required, ['principalId', 'kind', 'stewardPrincipalId', 'keys', 'name', 'fingerprint']);
  assert.deepEqual(principal.properties.kind.enum, ['person', 'agent']);
  assert.equal(principal.properties.keys.minItems, 1);
  assert.equal(principal.properties.keys.maxItems, 1);
  assert.equal(principal.properties.keys.items.$ref, '#/$defs/identity');
  assert.equal(schema.$defs.identity.pattern, '^[0-9a-f]{64}$');
  assert.equal(schema.$defs.principalId.pattern, '^[0-9a-f]{64}$');
});

test('only an agent has a steward', () => {
  const [rule] = schema.$defs.principal.allOf;
  assert.deepEqual(rule.if.properties.kind, { const: 'agent' });
  assert.deepEqual(rule.then.properties.stewardPrincipalId, { $ref: '#/$defs/principalId' });
  assert.deepEqual(rule.else.properties.stewardPrincipalId, { type: 'null' });
});

test('snapshot is bounded and the cursor is a decimal invalidation token', () => {
  const snapshot = schema.$defs.snapshot;
  closed(snapshot.properties, ['workspaceId', 'cursor', 'principals']);
  assert.equal(snapshot.properties.principals.maxItems, 1024);
  assert.equal(schema.$defs.cursor.pattern, '^(0|[1-9][0-9]*)$');
  assert.equal(schema.$defs.subscribe.properties.afterCursor.$ref, '#/$defs/cursor');
});

test('lookup selects by exactly one of identity or principalId', () => {
  const lookup = schema.$defs.lookupCall;
  assert.equal(lookup.additionalProperties, false);
  closed(lookup.properties, ['method', 'identity', 'principalId']);
  assert.deepEqual(lookup.required, ['method']);
  assert.deepEqual(lookup.oneOf.map((branch) => branch.required), [['identity'], ['principalId']]);
  assert.deepEqual(schema.$defs.lookupResult.required, ['principal']);
});
