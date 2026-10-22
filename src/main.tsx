import { NavigatorTheme } from "epub-navigator/ui";
import "@fontsource-variable/inter";
import "epub-navigator/react.css";
import "./styles/app.css";
import { createStore, Provider } from "jotai";
import { readBootstrapColorScheme, readBootstrapInteractionFeedback } from "./host/bootstrap/parse";
import ReactDOM from "react-dom/client";

const Root = __ELEPHANT_FIXTURE_RUNNER__
  ? (await import("./fixture-runner/runner")).default
  : (await import("./app/app")).default;

ReactDOM.createRoot(document.getElementById("root")!).render(
  <Provider store={createStore()}>
    <NavigatorTheme
      className="elephant-app"
      colorScheme={readBootstrapColorScheme()}
      interactionFeedback={readBootstrapInteractionFeedback()}
    >
      <Root />
    </NavigatorTheme>
  </Provider>,
);
