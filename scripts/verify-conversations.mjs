import { availableParallelism } from "node:os";
import { spawnSync } from "node:child_process";

const checks = [
  // Qualify the shipped artifact: the release Backend, the UI bundle and the
  // staged Package Directory. The system tests below build their own Backend
  // from source and no longer depend on this step (CR-012).
  ["npm", ["run", "build:conversations"]],
  // The hermetic run of the recovery matrix is already in `npm run check`.
  // This is the production-relay run of the same scenario: the realistic
  // addressing the hermetic one cannot reach (SF-002).
  [
    "cargo",
    [
      "test",
      "-p",
      "teamarness-system-tests",
      "--test",
      "conversations_two_profile_flow",
      "--locked",
      "complete_two_profile_recovery_matrix_over_production_relays",
      "--",
      "--ignored",
      "--exact",
      "--nocapture",
    ],
  ],
  [
    "cargo",
    [
      "test",
      "-p",
      "teamarness-system-tests",
      "--test",
      "conversations_two_profile_flow",
      "--locked",
      "ordinary_machine_qualification_meets_delivery_and_catch_up_targets",
      "--",
      "--ignored",
      "--exact",
      "--nocapture",
    ],
  ],
  [
    "cargo",
    [
      "test",
      "-p",
      "teamarness-system-tests",
      "--test",
      "conversations_three_profile_flow",
      "--locked",
      "three_profiles_converge_after_late_admission",
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
