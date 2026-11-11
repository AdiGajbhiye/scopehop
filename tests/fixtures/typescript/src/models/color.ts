export enum Color {
  Red,
  Green = "green",
}

export default class Palette {
  colors: Color[] = [];

  first(): Color {
    return this.colors[0];
  }
}
