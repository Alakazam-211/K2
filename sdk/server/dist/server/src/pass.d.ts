export declare class SecretFile {
    readonly path: string;
    constructor(path: string);
    /** Does the file exist now? */
    exists(): boolean;
    /** The current value. Throws when the file can't be read or is empty. */
    read(): string;
}
