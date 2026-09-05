export function shorten(text: string, max = 44): string {
  return text.length > max ? `${text.slice(0, max)}…` : text;
}
