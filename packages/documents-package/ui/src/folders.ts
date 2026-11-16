import type { Document } from "./contract.generated";

export type DocumentFolder = {
  path: string;
  name: string;
  documents: Document[];
  children: DocumentFolder[];
};

/** Canonical paths supply implicit parents; Documents keep the Backend's Order Key listing order. */
export function documentFolders(documents: readonly Document[]): DocumentFolder {
  const root: DocumentFolder = { path: "", name: "", documents: [], children: [] };
  const folders = new Map<string, DocumentFolder>([["", root]]);
  for (const document of documents) {
    let parent = root;
    for (const segment of document.folder === "" ? [] : document.folder.split("/")) {
      const path = parent.path ? `${parent.path}/${segment}` : segment;
      let folder = folders.get(path);
      if (!folder) {
        folder = { path, name: segment, documents: [], children: [] };
        folders.set(path, folder);
        parent.children.push(folder);
      }
      parent = folder;
    }
    parent.documents.push(document);
  }
  // Code-unit order is stable across locales and Installations.
  for (const folder of folders.values()) folder.children.sort((a, b) => a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
  return root;
}

export function folderAncestors(path: string): string[] {
  return path === "" ? [] : path.split("/").map((_, index, segments) => segments.slice(0, index + 1).join("/"));
}
