/** Minimal vscode mock for unit testing. */

export class Uri {
  static file(path: string) {
    return { scheme: "file", fsPath: path, path };
  }
  static parse(value: string) {
    return {
      scheme: "file",
      path: value,
      with: (change: Record<string, unknown>) => ({ ...change, path: value }),
    };
  }
}

export const workspace = {
  getConfiguration: () => ({
    get: (_key: string, defaultValue: unknown) => defaultValue,
  }),
  workspaceFolders: [],
};

export const window = {
  showErrorMessage: () => undefined,
  showWarningMessage: () => undefined,
  showInformationMessage: () => undefined,
  showInputBox: () => Promise.resolve(undefined),
  createTreeView: () => ({
    dispose: () => undefined,
    onDidChangeVisibility: () => ({ dispose: () => undefined }),
  }),
  createWebviewPanel: () => ({
    webview: { html: "" },
    reveal: () => undefined,
    onDidDispose: () => ({ dispose: () => undefined }),
    dispose: () => undefined,
  }),
  withProgress: async (_opts: unknown, task: (p: unknown) => Promise<unknown>) =>
    task({ report: () => undefined }),
};

export const commands = {
  registerCommand: (_cmd: string, _cb: unknown) => ({ dispose: () => undefined }),
  executeCommand: () => Promise.resolve(),
};

export const ProgressLocation = { Notification: 15 };

export class TreeItem {
  label: string;
  collapsibleState: number;
  contextValue?: string;
  description?: string;
  tooltip?: unknown;
  iconPath?: unknown;
  command?: unknown;

  constructor(label: string, collapsibleState?: number) {
    this.label = label;
    this.collapsibleState = collapsibleState ?? 0;
  }
}

export const TreeItemCollapsibleState = {
  None: 0,
  Collapsed: 1,
  Expanded: 2,
};

export class ThemeIcon {
  id: string;
  color?: unknown;
  constructor(id: string, color?: unknown) {
    this.id = id;
    this.color = color;
  }
}

export class ThemeColor {
  id: string;
  constructor(id: string) {
    this.id = id;
  }
}

export class MarkdownString {
  value: string;
  constructor(value?: string) {
    this.value = value ?? "";
  }
}

export class EventEmitter<T> {
  event = () => ({ dispose: () => undefined });
  fire(_data: T) {}
  dispose() {}
}

export enum ViewColumn {
  Active = -1,
  Beside = -2,
  One = 1,
}
