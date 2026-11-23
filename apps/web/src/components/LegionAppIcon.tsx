// The production icon, as GitHub shows the LegionCode app, whatever this build's channel.
import appIconUrl from "../../../../assets/prod/legioncode-black-web-apple-touch-180.png";

export function LegionAppIcon(props: { readonly className?: string }) {
  return <img src={appIconUrl} alt="" className={props.className} />;
}
