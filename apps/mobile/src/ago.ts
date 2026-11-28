/**
 * How long ago, as the drawer's rows say it (brand v5's P31-M): "now", then minutes, hours, days and weeks ("2m",
 * "3h", "1d", "2w"), and the date past a month ("Oct 3").
 */
export function ago(iso: string, now = Date.now()): string {
  const at = new Date(iso).getTime();
  if (!Number.isFinite(at)) return "";
  const minutes = Math.floor(Math.max(now - at, 0) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d`;
  if (days < 30) return `${Math.floor(days / 7)}w`;
  return new Date(at).toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

/** `ago` as VoiceOver and TalkBack read it (P94·E): "2 minutes", not "2m". */
export function spokenAgo(iso: string, now = Date.now()): string {
  const short = ago(iso, now);
  const unit = /^(\d+)([mhdw])$/.exec(short);
  if (!unit) return short === "now" ? "just now" : short;
  const count = Number(unit[1]);
  const word = { m: "minute", h: "hour", d: "day", w: "week" }[unit[2] as "m" | "h" | "d" | "w"];
  return `${count} ${word}${count === 1 ? "" : "s"}`;
}
