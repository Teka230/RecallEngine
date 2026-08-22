export type ViewMode = "current-thread" | "all-active";

export interface Page<T> {
  items: T[];
  next_cursor: string | null;
}

export interface MetaResponse {
  api_schema_version: string;
  database_schema_version: number;
  snapshot_id: string | null;
  database_path: string;
  read_only: boolean;
}

export interface Asset {
  id: string;
  name: string;
  mime_type: string | null;
  relative_path: string | null;
  exists_locally: boolean;
}

export interface BranchInfo {
  node_id: string;
  parent_id: string | null;
  is_current_thread: boolean | null;
}

export interface Message {
  message_id: string;
  ic: number | null;
  conversation_id: string;
  role: string;
  model_slug: string | null;
  content: string;
  created_at: number | null;
  timestamp: string | null;
  node_id: string;
  parent_id: string | null;
  technical: boolean;
  visible: boolean;
  branch: BranchInfo;
  reference: string | null;
  stable_reference: string | null;
  assets: Asset[];
}

export interface ConversationSummary {
  conversation_id: string;
  title: string;
  updated_at: number | null;
  message_count: number;
  default_model_slug: string | null;
  has_branches: boolean;
}

export interface ConversationDetail extends ConversationSummary {
  current_node_id: string | null;
  assets: Asset[];
}

export interface SearchHit extends Omit<Message, "content"> {
  conversation_title: string;
  excerpt: string;
}

export interface ContextResponse {
  view: ViewMode;
  before_requested: number;
  after_requested: number;
  center: Message;
  before: Message[];
  after: Message[];
}

export interface ReferenceResponse {
  requested_reference: string;
  reference: string;
  stable_reference: string;
  message: Message;
}

export interface IcResolution {
  ic: number;
  message_id: string;
  conversation_id: string;
  reference: string;
  stable_reference: string;
}
