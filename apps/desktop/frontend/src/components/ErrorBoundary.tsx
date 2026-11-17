import { Component, type ErrorInfo, type ReactNode } from "react";
import { errorMessage } from "../lib/format";
import { ErrorState } from "./ErrorState";

interface State {
  error: unknown;
}

/** Top-level safety net: a rendering bug shows a calm message instead of a blank window. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error: error ?? new Error("Unknown error") };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error("GameLink UI error", error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="grid h-full place-items-center p-8">
          <div className="w-full max-w-lg">
            <ErrorState
              title="Something went wrong in the interface"
              message={`${errorMessage(this.state.error)} Your servers and keys are not affected.`}
              onRetry={() => this.setState({ error: null })}
              retryLabel="Try again"
            />
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
