export function shout(s: string): string {
  return s.toUpperCase();
}

export function countWords(s: string): number {
  return s.split(" ").length;
}

export default function whisper(s: string): string {
  return s.toLowerCase();
}
