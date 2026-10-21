-- Schema 3: compact physical relations; public views preserve the path-ID read contract.


CREATE TABLE ai_models (
          id INTEGER PRIMARY KEY,
          kind TEXT NOT NULL,
          name TEXT NOT NULL,
          version TEXT,
          metadata_json TEXT,
          UNIQUE(kind, name, version)
        );

CREATE TABLE app_meta (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL,
          updated_at INTEGER NOT NULL
        );

CREATE TABLE folders (
          id INTEGER PRIMARY KEY,
          path TEXT NOT NULL UNIQUE,
          added_at INTEGER NOT NULL,
          last_scanned_at INTEGER,
          hidden INTEGER NOT NULL DEFAULT 0
        );

CREATE TABLE storage_image_ai_state (
          image_id INTEGER NOT NULL,
          model_id INTEGER NOT NULL,
          status TEXT NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, model_id),
          FOREIGN KEY(image_id) REFERENCES images(image_pk) ON DELETE CASCADE,
          FOREIGN KEY(model_id) REFERENCES ai_models(id) ON DELETE CASCADE
        ) WITHOUT ROWID;

CREATE TABLE storage_image_atmosphere_signatures (
          image_id INTEGER PRIMARY KEY,
          source_modified_at INTEGER NOT NULL,
          source_file_size INTEGER NOT NULL,
          signature_blob BLOB NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(image_id) REFERENCES images(image_pk) ON DELETE CASCADE
        );

CREATE TABLE storage_image_color_signatures (
          image_id INTEGER PRIMARY KEY,
          source_modified_at INTEGER NOT NULL,
          source_file_size INTEGER NOT NULL,
          signature_blob BLOB NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(image_id) REFERENCES images(image_pk) ON DELETE CASCADE
        );

CREATE TABLE storage_image_tags (
          image_id INTEGER NOT NULL,
          tag_id INTEGER NOT NULL,
          confidence REAL,
          source TEXT NOT NULL DEFAULT 'manual',
          PRIMARY KEY(image_id, tag_id, source),
          FOREIGN KEY(image_id) REFERENCES images(image_pk) ON DELETE CASCADE,
          FOREIGN KEY(tag_id) REFERENCES tags(id) ON DELETE CASCADE
        ) WITHOUT ROWID;

CREATE TABLE storage_image_thumbnails (
          image_id INTEGER PRIMARY KEY,
          thumb_path TEXT NOT NULL,
          source_modified_at INTEGER NOT NULL,
          source_file_size INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(image_id) REFERENCES images(image_pk) ON DELETE CASCADE
        );

CREATE TABLE storage_image_user_custom_tags (
          image_id INTEGER NOT NULL,
          tag_text TEXT NOT NULL,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, tag_text),
          FOREIGN KEY(image_id) REFERENCES images(image_pk) ON DELETE CASCADE
        ) WITHOUT ROWID;

CREATE TABLE storage_image_user_folders (
          image_id INTEGER NOT NULL,
          folder_id INTEGER NOT NULL,
          assigned_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, folder_id),
          FOREIGN KEY(image_id) REFERENCES images(image_pk) ON DELETE CASCADE,
          FOREIGN KEY(folder_id) REFERENCES user_folders(id) ON DELETE CASCADE
        ) WITHOUT ROWID;

CREATE TABLE storage_image_user_supplement_tags (
          image_id INTEGER NOT NULL,
          tag_en TEXT NOT NULL,
          tag_zh TEXT,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(image_id, tag_en),
          FOREIGN KEY(image_id) REFERENCES images(image_pk) ON DELETE CASCADE
        ) WITHOUT ROWID;

CREATE TABLE images (
          image_pk INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE,
          path TEXT NOT NULL UNIQUE,
          file_name TEXT NOT NULL,
          ext TEXT NOT NULL,
          width INTEGER NOT NULL,
          height INTEGER NOT NULL,
          file_size INTEGER NOT NULL,
          modified_at INTEGER NOT NULL,
          imported_at INTEGER NOT NULL,
          folder_id INTEGER NOT NULL,
          missing INTEGER NOT NULL DEFAULT 0,
          trashed INTEGER NOT NULL DEFAULT 0,
          is_favorite INTEGER NOT NULL DEFAULT 0,
          content_hash TEXT,
          source TEXT NOT NULL DEFAULT 'library',
          FOREIGN KEY(folder_id) REFERENCES folders(id) ON DELETE CASCADE
        );

-- Images whose insert transaction committed but whose initial directory
-- ownership was not applied yet. Recovery is per image, never per directory.
CREATE TABLE scan_pending_assignments (
          image_pk INTEGER PRIMARY KEY,
          created_at INTEGER NOT NULL,
          FOREIGN KEY(image_pk) REFERENCES images(image_pk) ON DELETE CASCADE
        );

CREATE TABLE known_image_tags (
          model_name TEXT NOT NULL,
          tag_en TEXT NOT NULL,
          tag_zh TEXT,
          image_count INTEGER NOT NULL DEFAULT 0,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(model_name, tag_en)
        );

CREATE TABLE reference_board_folders (
          id INTEGER PRIMARY KEY,
          name TEXT NOT NULL,
          sort_order INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        );

CREATE TABLE storage_reference_board_items (
          id INTEGER PRIMARY KEY,
          board_id INTEGER NOT NULL,
          image_id INTEGER NOT NULL,
          x REAL NOT NULL DEFAULT 0,
          y REAL NOT NULL DEFAULT 0,
          width REAL NOT NULL DEFAULT 220,
          height REAL NOT NULL DEFAULT 220,
          rotation REAL NOT NULL DEFAULT 0,
          flip_x INTEGER NOT NULL DEFAULT 0,
          flip_y INTEGER NOT NULL DEFAULT 0,
          z_index INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          FOREIGN KEY(board_id) REFERENCES reference_boards(id) ON DELETE CASCADE,
          FOREIGN KEY(image_id) REFERENCES images(image_pk) ON DELETE CASCADE
        );

CREATE TABLE reference_boards (
          id INTEGER PRIMARY KEY,
          folder_id INTEGER,
          name TEXT NOT NULL,
          sort_order INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(folder_id) REFERENCES reference_board_folders(id) ON DELETE SET NULL
        );

CREATE TABLE tag_dictionary (
          tag_en TEXT PRIMARY KEY,
          tag_zh TEXT NOT NULL,
          updated_at INTEGER NOT NULL
        );

CREATE TABLE tags (
          id INTEGER PRIMARY KEY,
          name TEXT NOT NULL,
          namespace TEXT,
          source TEXT NOT NULL DEFAULT 'manual',
          UNIQUE(name, namespace, source)
        );

CREATE TABLE task_runs (
                    id TEXT PRIMARY KEY, kind TEXT NOT NULL, state TEXT NOT NULL,
                    payload TEXT NOT NULL, updated_at INTEGER NOT NULL);

CREATE TABLE user_custom_tags (
          tag_text TEXT PRIMARY KEY,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        );

CREATE TABLE user_folder_rules (
          folder_id INTEGER PRIMARY KEY,
          rule_json TEXT NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(folder_id) REFERENCES user_folders(id) ON DELETE CASCADE
        );

CREATE TABLE user_folders (
          id INTEGER PRIMARY KEY,
          parent_id INTEGER,
          name TEXT NOT NULL,
          sort_order INTEGER NOT NULL DEFAULT 0,
          source_kind TEXT NOT NULL DEFAULT 'manual',
          source_path TEXT,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          FOREIGN KEY(parent_id) REFERENCES user_folders(id) ON DELETE CASCADE
        );

CREATE TABLE user_tag_folder_members (
          folder_id INTEGER NOT NULL,
          tag_text TEXT NOT NULL,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(folder_id, tag_text),
          UNIQUE(tag_text),
          FOREIGN KEY(folder_id) REFERENCES user_tag_folders(id) ON DELETE CASCADE
        );

CREATE TABLE user_tag_folders (
          id INTEGER PRIMARY KEY,
          name TEXT NOT NULL,
          sort_order INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        );

CREATE TABLE storage_strings (string_pk INTEGER PRIMARY KEY,value TEXT NOT NULL UNIQUE);
CREATE TABLE storage_auto_tags (
 image_pk INTEGER NOT NULL REFERENCES images(image_pk) ON DELETE CASCADE,
 model_pk INTEGER NOT NULL REFERENCES storage_strings(string_pk),
 category_pk INTEGER NOT NULL REFERENCES storage_strings(string_pk),
 tag_pk INTEGER NOT NULL REFERENCES storage_strings(string_pk),
 translation_pk INTEGER REFERENCES storage_strings(string_pk),
 confidence REAL NOT NULL,updated_at INTEGER NOT NULL,
 PRIMARY KEY(image_pk,model_pk,category_pk,tag_pk)
) WITHOUT ROWID;
CREATE TABLE storage_clip_models (model_pk INTEGER PRIMARY KEY,model_id TEXT NOT NULL,model_version TEXT NOT NULL,UNIQUE(model_id,model_version));
CREATE TABLE storage_clip_embeddings (
 image_id INTEGER NOT NULL REFERENCES images(image_pk) ON DELETE CASCADE,
 model_pk INTEGER NOT NULL REFERENCES storage_clip_models(model_pk),
 dimension INTEGER NOT NULL,normalized INTEGER NOT NULL DEFAULT 1,vector_blob BLOB NOT NULL,
 source_modified_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,
 PRIMARY KEY(image_id,model_pk)
);
CREATE VIEW image_auto_tags AS
 SELECT i.id AS image_id,c.value AS category,t.value AS tag_en,z.value AS tag_zh,a.confidence,m.value AS model_name,a.updated_at
 FROM storage_auto_tags a JOIN images i ON i.image_pk=a.image_pk
 JOIN storage_strings m ON m.string_pk=a.model_pk JOIN storage_strings c ON c.string_pk=a.category_pk
 JOIN storage_strings t ON t.string_pk=a.tag_pk LEFT JOIN storage_strings z ON z.string_pk=a.translation_pk;
CREATE VIEW image_clip_embeddings AS
 SELECT i.id AS image_id,m.model_id,m.model_version,e.dimension,e.normalized,e.vector_blob,e.source_modified_at,e.updated_at
 FROM storage_clip_embeddings e JOIN images i ON i.image_pk=e.image_id JOIN storage_clip_models m ON m.model_pk=e.model_pk;

CREATE VIEW image_ai_state AS SELECT i.id AS image_id,s.model_id,s.status,s.updated_at FROM storage_image_ai_state s JOIN images i ON i.image_pk=s.image_id;

CREATE VIEW image_atmosphere_signatures AS SELECT i.id AS image_id,s.source_modified_at,s.source_file_size,s.signature_blob,s.updated_at FROM storage_image_atmosphere_signatures s JOIN images i ON i.image_pk=s.image_id;

CREATE VIEW image_color_signatures AS SELECT i.id AS image_id,s.source_modified_at,s.source_file_size,s.signature_blob,s.updated_at FROM storage_image_color_signatures s JOIN images i ON i.image_pk=s.image_id;

CREATE VIEW image_tags AS SELECT i.id AS image_id,s.tag_id,s.confidence,s.source FROM storage_image_tags s JOIN images i ON i.image_pk=s.image_id;

CREATE VIEW image_thumbnails AS SELECT i.id AS image_id,s.thumb_path,s.source_modified_at,s.source_file_size,s.updated_at FROM storage_image_thumbnails s JOIN images i ON i.image_pk=s.image_id;

CREATE VIEW image_user_custom_tags AS SELECT i.id AS image_id,s.tag_text,s.created_at,s.updated_at FROM storage_image_user_custom_tags s JOIN images i ON i.image_pk=s.image_id;

CREATE VIEW image_user_folders AS SELECT i.id AS image_id,s.folder_id,s.assigned_at FROM storage_image_user_folders s JOIN images i ON i.image_pk=s.image_id;

CREATE VIEW image_user_supplement_tags AS SELECT i.id AS image_id,s.tag_en,s.tag_zh,s.created_at,s.updated_at FROM storage_image_user_supplement_tags s JOIN images i ON i.image_pk=s.image_id;

CREATE VIEW reference_board_items AS SELECT s.id,s.board_id,i.id AS image_id,s.x,s.y,s.width,s.height,s.rotation,s.flip_x,s.flip_y,s.z_index,s.created_at FROM storage_reference_board_items s JOIN images i ON i.image_pk=s.image_id;
