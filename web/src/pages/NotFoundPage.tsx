import { useDocumentTitle } from '../lib/useDocumentTitle';
import { FullScreenMessage } from '../components/StatusScreens';

export default function NotFoundPage() {
  useDocumentTitle('Not found');
  return (
    <FullScreenMessage
      title="Page not found"
      message="The page you're looking for doesn't exist or was moved."
      homeLink
    />
  );
}
