import { Component, type ReactNode, type ErrorInfo } from "react";

interface ErrorBoundaryProps {
  /** Label shown in the fallback UI to identify which panel crashed. */
  panelName: string;
  children: ReactNode;
}

interface ErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
}

/**
 * React Error Boundary — catches render errors in child components and shows
 * a fallback UI instead of crashing the entire app.
 *
 * Wraps each panel (DiffViewer, FlowGraph, RiskHeatmap, annotations) so that
 * a crash in one panel doesn't take down the whole application.
 */
export default class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Log to console for debugging — avoid console.error in tests
    if (typeof window !== "undefined" && !(window as unknown as Record<string, unknown>).__TEST_API__) {
      // eslint-disable-next-line no-console
      console.error(`[ErrorBoundary:${this.props.panelName}]`, error, info.componentStack);
    }
  }

  handleReset = () => {
    this.setState({ hasError: false, error: null });
  };

  render() {
    if (this.state.hasError) {
      return (
        <div className="error-boundary-fallback" data-testid={`error-boundary-${this.props.panelName}`}>
          <div className="error-boundary-icon">&#9888;</div>
          <div className="error-boundary-title">{this.props.panelName} crashed</div>
          <div className="error-boundary-message">
            {this.state.error?.message || "An unexpected error occurred"}
          </div>
          <button className="btn error-boundary-retry" onClick={this.handleReset}>
            Retry
          </button>
        </div>
      );
    }

    return this.props.children;
  }
}
