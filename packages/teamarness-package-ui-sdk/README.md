# Teamarness Package UI SDK

`@teamarness/package-ui-sdk` is the TypeScript contract for a Package UI mounted by the Teamarness Shell. It provides the mount-scoped Package client, the authoritative theme-token names, and an optional framework-neutral CSS kit.

A Package owns everything it renders inside its Shadow Root. The Shell supplies the mount and inherited semantic values; it does not own a Package-owned header, secondary navigation, content hierarchy, dialogs, tables, or domain patterns. The CSS kit styles only classes a Package explicitly applies.

## Theme tokens

`TEAMARNESS_THEME_TOKENS` is the exact frozen token-name authority. `TeamarnessThemeToken` is derived from that constant, and `PackageMountContext.themeTokens` is typed with it. The Shell defines values on the Package mount ancestor, so changes inherit into the Shadow Root without a theme event.

| Token | Intended use |
|---|---|
| `--teamarness-color-background` | Main Package surface and input backgrounds |
| `--teamarness-color-panel` | Raised or grouped surfaces |
| `--teamarness-color-text` | Primary text |
| `--teamarness-color-text-muted` | Secondary and help text |
| `--teamarness-color-border` | Dividers and neutral control borders |
| `--teamarness-color-accent` | Primary actions and selected emphasis |
| `--teamarness-color-danger` | Errors, destructive emphasis, and validation |
| `--teamarness-color-focus` | Visible keyboard-focus outlines |
| `--teamarness-font-body` | Shell-compatible body font stack |
| `--teamarness-font-mono` | Values that benefit from monospaced alignment |
| `--teamarness-radius-control` | Controls, inputs, and notices |
| `--teamarness-radius-panel` | Larger grouped surfaces |

Use the semantic purpose rather than assuming a particular light or dark value:

```css
:host {
  color: var(--teamarness-color-text);
  background: var(--teamarness-color-background);
  font-family: var(--teamarness-font-body);
}
```

## Optional CSS kit

Call `installPackageUiKit(root)` during mount. It constructs one `CSSStyleSheet` and adds it to that Shadow Root's `adoptedStyleSheets`, even when installers overlap. Keep the returned cleanup function and call it during unmount. Each cleanup is idempotent. The sheet remains until every overlapping installer has cleaned up, and cleanup preserves every unrelated adopted sheet.

```ts
import { installPackageUiKit, type PackageMountContext } from "@teamarness/package-ui-sdk";

export function mount(context: PackageMountContext): () => void {
  const cleanupKit = installPackageUiKit(context.root);
  const content = document.createElement("section");
  content.className = "teamarness-panel teamarness-stack";
  context.root.append(content);

  let unmounted = false;
  return () => {
    if (unmounted) return;
    unmounted = true;
    content.remove();
    cleanupKit();
  };
}
```

The kit publishes these opt-in classes:

| Class | Purpose |
|---|---|
| `.teamarness-control` | Common button-like control foundation |
| `.teamarness-control--primary` | Primary-action treatment, used with the control foundation |
| `.teamarness-control--secondary` | Secondary-action treatment, used with the control foundation |
| `.teamarness-field` | Vertical field-label, input, and help flow |
| `.teamarness-input` | Textual input surface |
| `.teamarness-help` | Muted field guidance |
| `.teamarness-validation` | Validation or error text |
| `.teamarness-notice` | Status or informational notice |
| `.teamarness-notice--danger` | Danger treatment, used with the notice foundation |
| `.teamarness-panel` | Grouped Package-owned surface |
| `.teamarness-stack` | Vertical flow with a standard gap |
| `.teamarness-inline` | Wrapping horizontal flow with a standard gap |
| `.teamarness-focus` | Visible `:focus-visible` outline for a custom focusable element |

The installer adds only local box sizing outside those classes. It does not select or modify Package content, and it does not install styles in `document`.

The host's `style-src` rejects runtime inline `<style>` elements and `style` attributes. The installer does not bypass that policy. It fails the Package mount if constructed stylesheets or mutable `ShadowRoot.adoptedStyleSheets` are unavailable. Do not add an inline fallback, nonce, hash, `data:` style, or `blob:` style.

Package-owned CSS must follow the same `style-src` policy. Construct a separate `CSSStyleSheet`, call `replaceSync()`, add it to the Package Shadow Root's `adoptedStyleSheets`, and remove only that sheet during unmount. Keep Package rules separate from the optional kit so ownership and cleanup remain clear.

## Package-owned composition

Build the Package's information architecture in Package markup. For example, this Package-owned header and secondary navigation use tokens and flow helpers without asking the Shell to interpret Package routes:

```html
<article class="teamarness-panel teamarness-stack">
  <header class="package-header teamarness-stack">
    <p class="teamarness-help">Planning</p>
    <h1>Milestones</h1>
    <nav class="teamarness-inline" aria-label="Milestone sections">
      <button class="teamarness-control teamarness-control--primary" aria-current="page">Board</button>
      <button class="teamarness-control teamarness-control--secondary">Activity</button>
    </nav>
  </header>
  <section aria-labelledby="current-milestone">
    <h2 id="current-milestone">Current milestone</h2>
    <!-- Package-owned domain content -->
  </section>
</article>
```

The Shell continues to own Workspace and Package selection, the right-pane mount, inherited token values, and generic mount recovery. Hiding or unmounting this UI does not suspend its Package Backend. A Shadow Root provides style locality, not a security boundary; Package UI code shares the webview's DOM and known Tauri-command authority.
