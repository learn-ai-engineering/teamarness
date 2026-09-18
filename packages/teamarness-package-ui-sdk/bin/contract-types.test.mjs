import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { generateContractTypes, parseArguments, run } from "./contract-types.mjs";

const run_ = promisify(execFile);
const packageRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repositoryRoot = dirname(dirname(packageRoot));

const SMALL_CONTRACT = {
  $defs: {
    empty: { type: "object", properties: {}, additionalProperties: false },
    status: { type: "string", enum: ["ready", "waiting"] },
    item: {
      type: "object",
      additionalProperties: false,
      required: ["id", "status"],
      properties: {
        id: { type: "string" },
        status: { $ref: "#/$defs/status" },
        note: { type: "string" },
      },
    },
  },
  requests: {
    "demo.read": {
      request: { $ref: "#/$defs/empty" },
      response: {
        type: "object",
        additionalProperties: false,
        required: ["items", "count"],
        properties: {
          items: { type: "array", items: { $ref: "#/$defs/item" } },
          count: { type: "integer", minimum: 0 },
        },
      },
    },
  },
  subscriptions: {
    "demo.changes": {
      request: { $ref: "#/$defs/empty" },
      event: { anyOf: [{ const: 1 }, { type: "null" }] },
    },
  },
};

const SMALL_EXPECTED = `// Generated from ui-contract.json by teamarness-contract-types. Do not edit.

export type Empty = Record<string, never>;

export type Item = {
  readonly "id": string;
  readonly "note"?: string;
  readonly "status": Status;
};

export type Status = "ready" | "waiting";

export type DemoContract = {
  readonly requests: {
    readonly "demo.read": { readonly request: Empty; readonly response: {
        readonly "count": number;
        readonly "items": readonly Item[];
      } };
  };
  readonly subscriptions: {
    readonly "demo.changes": { readonly request: Empty; readonly event: 1 | null };
  };
};
`;

test("generates closed object, enum, const, array, union and reference types", () => {
  assert.equal(generateContractTypes(SMALL_CONTRACT, { typeName: "DemoContract" }), SMALL_EXPECTED);
});

test("output is deterministic regardless of key order", () => {
  const reordered = {
    subscriptions: SMALL_CONTRACT.subscriptions,
    requests: SMALL_CONTRACT.requests,
    $defs: {
      status: SMALL_CONTRACT.$defs.status,
      item: SMALL_CONTRACT.$defs.item,
      empty: SMALL_CONTRACT.$defs.empty,
    },
  };
  assert.equal(generateContractTypes(reordered, { typeName: "DemoContract" }), SMALL_EXPECTED);
});

test("falls back to PackageValue for unsupported keywords and open objects", () => {
  const generated = generateContractTypes(
    {
      $defs: {
        open: { type: "object", properties: { id: { type: "string" } } },
        constrained: { minLength: 3 },
        loose: { type: "object", additionalProperties: true },
        typed: { type: "object", additionalProperties: { type: "number" } },
      },
      requests: {},
      subscriptions: {},
    },
    { typeName: "FallbackContract" },
  );
  assert.match(generated, /import type \{ PackageValue \} from "@teamarness\/package-ui-sdk";/u);
  assert.match(generated, /export type Constrained = PackageValue;/u);
  assert.match(generated, /readonly \[key: string\]: PackageValue;/u);
  assert.match(generated, /readonly \[key: string\]: number;/u);
  assert.match(generated, /readonly requests: Record<string, never>;/u);
});

test("names collide deterministically and never start with a digit", () => {
  const generated = generateContractTypes(
    {
      $defs: {
        "my-value": { type: "string" },
        my_value: { type: "number" },
        "1st": { type: "boolean" },
      },
      requests: {},
      subscriptions: {},
    },
    { typeName: "CollisionContract" },
  );
  assert.match(generated, /export type T1st = boolean;/u);
  assert.match(generated, /export type MyValue = string;/u);
  assert.match(generated, /export type MyValue2 = number;/u);
});

test("parses its arguments and refuses incomplete ones", () => {
  assert.deepEqual(
    parseArguments(["--contract", "a.json", "--out", "b.ts", "--type-name", "C", "--check"]),
    { check: true, contract: "a.json", out: "b.ts", typeName: "C" },
  );
  assert.throws(() => parseArguments(["--contract", "a.json"]), /Missing required argument: --out/u);
  assert.throws(() => parseArguments(["--nope", "x"]), /Unknown argument: --nope/u);
});

test("check mode reports stale output with exit code 1", async () => {
  const directory = await mkdtemp(join(packageRoot, ".contract-types-check-"));
  try {
    const contract = join(directory, "ui-contract.json");
    const out = join(directory, "contract.generated.ts");
    await writeFile(contract, JSON.stringify(SMALL_CONTRACT));

    assert.equal(
      await run(["--contract", contract, "--out", out, "--type-name", "DemoContract", "--check"]),
      1,
      "a missing file is stale",
    );
    assert.equal(
      await run(["--contract", contract, "--out", out, "--type-name", "DemoContract"]),
      0,
    );
    assert.equal(await readFile(out, "utf8"), SMALL_EXPECTED);
    assert.equal(
      await run(["--contract", contract, "--out", out, "--type-name", "DemoContract", "--check"]),
      0,
      "freshly written output is current",
    );

    await writeFile(out, `${SMALL_EXPECTED}// edited\n`);
    assert.equal(
      await run(["--contract", contract, "--out", out, "--type-name", "DemoContract", "--check"]),
      1,
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("generated types typecheck against the SDK client", async (t) => {
  const directory = await mkdtemp(join(packageRoot, ".contract-types-check-"));
  try {
    const source = join(directory, "src");
    await mkdir(source, { recursive: true });
    await run([
      "--contract",
      join(repositoryRoot, "packages/tutorial/ui-contract.json"),
      "--out",
      join(source, "contract.generated.ts"),
      "--type-name",
      "TutorialContract",
    ]);
    await writeFile(
      join(source, "usage.ts"),
      `import { typedPackageClient, type PackageClient } from "@teamarness/package-ui-sdk";\n`
        + `import type { TutorialContract } from "./contract.generated";\n\n`
        + `export async function read(client: PackageClient): Promise<number> {\n`
        + `  const typed = typedPackageClient<TutorialContract>(client);\n`
        + `  const response = await typed.request("tutorial.read", {});\n`
        + `  return response.artworks.length;\n`
        + `}\n`,
    );
    await writeFile(
      join(directory, "tsconfig.json"),
      JSON.stringify({
        compilerOptions: {
          target: "ESNext",
          module: "ESNext",
          moduleResolution: "Bundler",
          strict: true,
          noEmit: true,
          lib: ["ESNext", "DOM"],
        },
        include: ["src"],
      }),
    );

    await run_("npx", ["tsc", "--noEmit", "-p", directory], { cwd: repositoryRoot });

    // The same client refuses an operation the contract does not describe.
    await writeFile(
      join(source, "usage.ts"),
      `import { typedPackageClient, type PackageClient } from "@teamarness/package-ui-sdk";\n`
        + `import type { TutorialContract } from "./contract.generated";\n\n`
        + `export async function nope(client: PackageClient): Promise<unknown> {\n`
        + `  return typedPackageClient<TutorialContract>(client).request("tutorial.nope", {});\n`
        + `}\n`,
    );
    const failure = await run_("npx", ["tsc", "--noEmit", "-p", directory], {
      cwd: repositoryRoot,
    }).then(
      () => null,
      (error) => error,
    );
    assert.ok(failure !== null, "an unknown operation must not typecheck");
    assert.match(`${failure.stdout ?? ""}${failure.stderr ?? ""}`, /error TS2345/u);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
  t.diagnostic("typechecked the generated Tutorial contract types");
});
