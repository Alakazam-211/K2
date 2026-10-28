import { readFile } from "node:fs/promises";

type Greeting = { lang: string; text: string };

const greetings: Greeting[] = [
	{ lang: "en", text: "Hello" },
	{ lang: "ja", text: "こんにちは" },
	{ lang: "ar", text: "مرحبا" },
	{ lang: "emoji", text: "👋🏽 👩‍👩‍👧‍👦" },
];

export async function load(path: string): Promise<string[]> {
	const body = await readFile(path, "utf8");
	return body.split(/\r?\n/).filter((l) => l.trim() !== "");
}

export const pick = (lang: string): string =>
	greetings.find((g) => g.lang === lang)?.text ?? `no greeting for ${lang}`;
