import { adoptPackageStyles, installPackageUiKit, typedPackageClient } from "@teamarness/package-ui-sdk";
import conversationsCss from "./styles.css?raw";
// Retain the CSS asset declared by the Package manifest; the Frame adopts the raw CSS below.
import "./styles.css";
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
  let cleanupStyles: () => void;
  try {
    cleanupStyles = adoptPackageStyles(context.root, conversationsCss);
  } catch (error) {
    cleanupKit();
    throw error;
  }
  let disposed = false;
  let nameRetry: ReturnType<typeof setTimeout> | undefined;
  let nameSubscription: Awaited<ReturnType<typeof client.subscribe>> | null = null;
  let nameRetryDelay = 1000;
  let view: ConversationsView | undefined;
  const controller = new ConversationsController(client, () => {
    if (!disposed) view?.render();
  });
  try {
    view = new ConversationsView(context.root, controller, context.package.displayName);
    await controller.initialize();
    view.render();

    // Subscribe to live snapshot updates.
    const subscription = await client.subscribe(
      "conversations.changes",
      {},
      async (snapshot) => {
        if (disposed) return;
        await controller.applySnapshot(snapshot);
        view?.render();
      },
    );

    await controller.catchUpAfterSubscribe();

    // Names are optional presentation: a name stream failure must not prevent Messages.
    const scheduleNameRetry = () => {
      if (disposed || nameRetry !== undefined) return;
      nameSubscription = null;
      nameRetry = setTimeout(() => {
        nameRetry = undefined;
        void openNames();
      }, nameRetryDelay);
      nameRetryDelay = Math.min(nameRetryDelay * 2, 30_000);
    };
    const openNames = async (): Promise<void> => {
      let terminated = false;
      try {
        const opened = await client.subscribe("conversations.names", {}, (names) => {
          if (!disposed) {
            nameRetryDelay = 1000;
            controller.applyNames(names);
          }
        }, { onTerminated: () => {
          terminated = true;
          scheduleNameRetry();
        } });
        if (disposed || terminated) await opened.close();
        else nameSubscription = opened;
      } catch {
        scheduleNameRetry();
      }
    };
    await openNames();

    return async () => {
      if (disposed) return;
      disposed = true;
      if (nameRetry) clearTimeout(nameRetry);
      controller.dispose();
      view?.dispose();
      try {
        try {
          await subscription.close();
        } finally {
          await nameSubscription?.close();
        }
      } finally {
        cleanupStyles();
        cleanupKit();
      }
    };
  } catch (error) {
    disposed = true;
    if (nameRetry) clearTimeout(nameRetry);
    controller.dispose();
    view?.dispose();
    cleanupStyles();
    cleanupKit();
    throw error;
  }
}
