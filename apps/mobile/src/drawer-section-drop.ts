export type Point = { x: number; y: number };
export type DropRect = Point & { width: number; height: number };

function contains(rect: DropRect, point: Point): boolean {
  return point.x >= rect.x && point.x < rect.x + rect.width &&
    point.y >= rect.y && point.y < rect.y + rect.height;
}

export function dropSection(
  pointer: Point,
  root: DropRect,
  visible: DropRect,
  sections: ReadonlyMap<string, DropRect>,
  current: string | undefined,
  names: ReadonlySet<string>,
): string | undefined {
  if (!contains(visible, pointer) || !contains(root, pointer)) return undefined;
  const local = { x: pointer.x - root.x, y: pointer.y - root.y };
  for (const [name, bounds] of sections) {
    if (name !== current && names.has(name) && contains(bounds, local)) return name;
  }
  return undefined;
}
