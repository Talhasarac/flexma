import { Component, type ErrorInfo, type ReactNode } from 'react';
import { FullScreenMessage } from './StatusScreens';

interface Props {
  children: ReactNode;
  /** Changing this resets the boundary (e.g. the current pathname). */
  resetKey?: string;
}

interface State {
  error: Error | null;
  resetKey?: string;
}

/** Catches render errors (and failed lazy chunk loads) so the app never goes blank. */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, resetKey: this.props.resetKey };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  static getDerivedStateFromProps(props: Props, state: State): Partial<State> | null {
    if (props.resetKey !== state.resetKey) return { error: null, resetKey: props.resetKey };
    return null;
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('Flexma UI error:', error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    const chunkFailed = /dynamically imported module|Loading chunk|Importing a module script failed/i.test(error.message);
    return (
      <FullScreenMessage
        title={chunkFailed ? 'Flexma was updated' : 'Something went wrong'}
        message={
          chunkFailed
            ? 'A newer version is available. Reload to continue.'
            : error.message || 'An unexpected error occurred.'
        }
        action={{ label: 'Reload', onClick: () => window.location.reload() }}
        homeLink
      />
    );
  }
}
