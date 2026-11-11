import { Circle } from "@/models";

type Props = { circle: Circle; label?: string };

export function Badge({ circle, label }: Props) {
  const size = circle.radius;
  return <div title={label}>{size}</div>;
}

export function App() {
  const label = "big";
  return <Badge circle={Circle.unit()} label={label} />;
}
