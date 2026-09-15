import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";

export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const article = document.createElement("article");
  const heading = document.createElement("h1");
  heading.textContent = context.package.displayName;
  const message = document.createElement("p");
  message.textContent = "Loading…";
  article.append(heading, message);
  context.root.append(article);

  const response = await context.client.request<{ message: string }>("__REQUEST_NAMESPACE__.read", {});
  message.textContent = response.message;

  return () => article.remove();
}
