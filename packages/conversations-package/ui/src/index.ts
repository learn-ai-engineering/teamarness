import { installPackageUiKit, typedPackageClient } from "@teamarness/package-ui-sdk";
import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";
import type {
  Availability,
  Channel,
  ConversationsContract,
  Message,
  Page,
  Snapshot,
} from "./contract.generated";
import { ConversationsController } from "./controller";
import { ConversationsView } from "./view";

export type { Availability, ConversationsContract };
export type ConversationChannel = Channel;
export type ConversationMessage = Message;
export type ConversationSnapshot = Snapshot;
export type ConversationPage = Page;

export async function mount(
  context: PackageMountContext,
): Promise<PackageUnmount> {
  const client = typedPackageClient<ConversationsContract>(context.client);
  const cleanupKit = installPackageUiKit(context.root);
  let disposed = false;

  const controller = new ConversationsController(client, () => {
    if (!disposed) view.render();
  });
  const view = new ConversationsView(context.root, controller, context.package.displayName);

  await controller.initialize();
  view.render();

  // Subscribe to live snapshot updates.
  const subscription = await client.subscribe(
    "conversations.changes",
    {},
    async (snapshot) => {
      if (disposed) return;
      await controller.applySnapshot(snapshot);
      view.render();
    },
  );

  return async () => {
    if (disposed) return;
    disposed = true;
    controller.dispose();
    view.dispose();
    await subscription.close();
    cleanupKit();
  };
}