import { installPackageUiKit } from "@teamarness/package-ui-sdk";
import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";
import { ConversationsController } from "./controller";
import { ConversationsView } from "./view";

export type ConversationChannel = Readonly<{
  id: string;
  slug: string;
  canHide: boolean;
}>;
export type ConversationMessage = Readonly<{
  id: string;
  author: string;
  markdown: string;
  isLocal?: boolean;
}>;
export type ConversationSnapshot = Readonly<{
  channels: readonly ConversationChannel[];
  availability?: string;
  warnings?: readonly string[];
}>;
export type ConversationPage = Readonly<{
  messages: readonly ConversationMessage[];
  nextBefore?: string;
}>;

export async function mount(
  context: PackageMountContext,
): Promise<PackageUnmount> {
  const cleanupKit = installPackageUiKit(context.root);
  let disposed = false;

  const controller = new ConversationsController(context.client, () => {
    if (!disposed) view.render();
  });
  const view = new ConversationsView(context.root, controller);

  await controller.initialize();
  view.render();

  // Subscribe to live snapshot updates.
  const subscription = await context.client.subscribe<ConversationSnapshot>(
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