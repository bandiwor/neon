export interface SourceFile {
  readonly name: string;
  readonly text: string;
}

export interface Project {
  readonly files: readonly SourceFile[];
  /** The file that is open in the editor. */
  readonly active: string;
  /** The file that is run. */
  readonly entry: string;
}

export interface Example {
  readonly id: string;
  readonly group: string;
  readonly title: string;
  readonly description: string;
  readonly interactive: boolean;
  readonly files: readonly SourceFile[];
}
