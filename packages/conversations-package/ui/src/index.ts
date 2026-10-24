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
  let principalRetry: ReturnType<typeof setTimeout> | undefined;
  let principalSubscription: Awaited<ReturnType<typeof client.subscribe>> | null = null;
  let principalRetryDelay = 1000;
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

    // Principals are optional presentation: a Principal stream failure must not prevent Messages.
    const schedulePrincipalRetry = () => {
      if (disposed || principalRetry !== undefined) return;
      principalSubscription = null;
      principalRetry = setTimeout(() => {
        principalRetry = undefined;
        void openPrincipals();
      }, principalRetryDelay);
      principalRetryDelay = Math.min(principalRetryDelay * 2, 30_000);
    };
    const openPrincipals = async (): Promise<void> => {
      let terminated = false;
      try {
        const opened = await client.subscribe("conversations.principals", {}, (principals) => {
          if (!disposed) {
            principalRetryDelay = 1000;
            controller.applyPrincipals(principals);
          }
        }, { onTerminated: () => {
          terminated = true;
          schedulePrincipalRetry();
        } });
        if (disposed || terminated) await opened.close();
        else principalSubscription = opened;
      } catch {
        schedulePrincipalRetry();
      }
    };
    await openPrincipals();

    return async () => {
      if (disposed) return;
      disposed = true;
      if (principalRetry) clearTimeout(principalRetry);
      controller.dispose();
      view?.dispose();
      try {
        try {
          await subscription.close();
        } finally {
          await principalSubscription?.close();
        }
      } finally {
        cleanupStyles();
        cleanupKit();
      }
    };
  } catch (error) {
    disposed = true;
    if (principalRetry) clearTimeout(principalRetry);
    controller.dispose();
    view?.dispose();
    cleanupStyles();
    cleanupKit();
    throw error;
  }
}
