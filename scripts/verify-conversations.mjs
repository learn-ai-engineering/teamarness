import { availableParallelism } from "node:os";
import { spawnSync } from "node:child_process";

const checks = [
  [
    "cargo",
    [
      "test",
      "-p",
      "teamarness-conversations-package",
      "--test",
      "two_profile_flow",
      "--locked",
      "complete_two_profile_recovery_matrix",
      "--",
      "--exact",
      "--nocapture",
    ],
  ],
  [
    "cargo",
    [
      "test",
      "-p",
      "teamarness-conversations-package",
      "--test",
      "two_profile_flow",
      "--locked",
      "ordinary_machine_qualification_meets_delivery_and_catch_up_targets",
      "--",
      "--ignored",
      "--exact",
      "--nocapture",
    ],
  ],
  ["cargo", ["test", "-p", "teamarness-conversations-package", "--lib", "--locked"]],
  ["cargo", ["test", "-p", "teamarness-core", "--test", "immutable_record_recovery", "--locked"]],
  [
    process.execPath,
    [
      "--test",
      "scripts/build-application-packages.test.mjs",
      "scripts/package-foundation-qualification.test.mjs",
    ],
  ],
  ["npm", ["test", "--workspace", "@teamarness/conversations-package-ui"]],
];

console.log(
  `Conversations qualification environment: platform=${process.platform} arch=${process.arch} node=${process.version} cpus=${availableParallelism()}`,
);

for (const [command, args] of checks) {
  console.log(`\n> ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, {
    cwd: new URL("../", import.meta.url),
    stdio: "inherit",
    env: { ...process.env, NO_COLOR: "1" },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

console.log("\nConversations qualification passed.");
