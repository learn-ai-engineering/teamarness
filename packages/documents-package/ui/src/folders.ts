import type { Document } from "./contract.generated";

export type DocumentFolder = {
  path: string;
  name: string;
  documents: Document[];
  children: DocumentFolder[];
};

// Compare case-insensitively without locale variation, then break ties by original spelling.
function compareLabels(a: string, b: string): number {
  const lowerA = a.toLowerCase();
  const lowerB = b.toLowerCase();
  if (lowerA !== lowerB) return lowerA < lowerB ? -1 : 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Canonical paths supply implicit parents; only the derived sibling arrays are sorted. */
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
  for (const folder of folders.values()) {
    folder.children.sort((a, b) => compareLabels(a.name, b.name));
    folder.documents.sort((a, b) => compareLabels(a.title, b.title) || compareLabels(a.id, b.id));
  }
  return root;
}

export function folderAncestors(path: string): string[] {
  return path === "" ? [] : path.split("/").map((_, index, segments) => segments.slice(0, index + 1).join("/"));
}
