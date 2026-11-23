import type { ColorValue } from "react-native";
import Svg, { Path } from "react-native-svg";
import { withUniwind } from "uniwind";

const ThemedPath = withUniwind(Path);

export function LegionWordmark(props: {
  readonly height: number;
  readonly color?: ColorValue;
  readonly colorClassName?: string;
}) {
  const aspectRatio = 104 / 78;
  return (
    <Svg
      accessibilityLabel="LegionCode"
      height={props.height}
      width={props.height * aspectRatio}
      viewBox="8 21 104 78"
    >
      <ThemedPath
        d="M12 25 H28 V79 H56 V95 H22 L12 85 Z M72 25 H108 V41 H78 V79 H108 V95 H72 L62 85 V35 Z"
        color={props.color}
        colorClassName={props.colorClassName}
        fill="currentColor"
      />
    </Svg>
  );
}
