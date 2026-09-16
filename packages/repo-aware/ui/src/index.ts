import type {
  PackageMountContext,
  PackageUnmount,
} from "@teamarness/package-ui-sdk";

export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const article = document.createElement("article");
  const heading = document.createElement("h2");
  heading.textContent = context.package.displayName;
  const description = document.createElement("p");
  description.textContent = "Register a Git repository to get started.";
  article.append(heading, description);
  context.root.append(article);

  let unmounted = false;
  return () => {
    if (unmounted) return;
    unmounted = true;
    article.remove();
  };
}