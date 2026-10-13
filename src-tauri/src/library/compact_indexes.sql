CREATE INDEX storage_idx_image_atmosphere_signatures_updated
          ON storage_image_atmosphere_signatures(updated_at DESC);

CREATE INDEX storage_idx_image_color_signatures_updated
          ON storage_image_color_signatures(updated_at DESC);

CREATE INDEX storage_idx_image_thumbnails_updated_at
          ON storage_image_thumbnails(updated_at DESC);

CREATE INDEX storage_idx_image_user_custom_tags_image_id
          ON storage_image_user_custom_tags(image_id, updated_at DESC);

CREATE INDEX storage_idx_image_user_folders_folder_id ON storage_image_user_folders(folder_id);

CREATE INDEX storage_idx_image_user_supplement_tags_image_id
          ON storage_image_user_supplement_tags(image_id, updated_at DESC);

CREATE INDEX idx_images_folder_id ON images(folder_id);

CREATE INDEX idx_images_modified_at ON images(modified_at DESC);

CREATE INDEX idx_known_image_tags_model_en
          ON known_image_tags(model_name, tag_en);

CREATE INDEX idx_known_image_tags_model_zh
          ON known_image_tags(model_name, tag_zh);

CREATE INDEX storage_idx_reference_board_items_board_id
          ON storage_reference_board_items(board_id, z_index);

CREATE INDEX idx_reference_boards_folder_id
          ON reference_boards(folder_id);

CREATE INDEX idx_user_folders_parent_id ON user_folders(parent_id);

CREATE UNIQUE INDEX idx_user_folders_source_path
          ON user_folders(source_path)
          WHERE source_path IS NOT NULL;

CREATE INDEX idx_user_tag_folder_members_folder_id
          ON user_tag_folder_members(folder_id, updated_at DESC);

CREATE INDEX storage_auto_tags_reverse ON storage_auto_tags(model_pk,tag_pk,image_pk,confidence);

CREATE INDEX storage_clip_model_updated ON storage_clip_embeddings(model_pk,updated_at DESC);

CREATE TRIGGER storage_auto_tags_insert_known AFTER INSERT ON storage_auto_tags
 WHEN NOT EXISTS(SELECT 1 FROM storage_auto_tags t WHERE t.image_pk=NEW.image_pk AND t.model_pk=NEW.model_pk AND t.tag_pk=NEW.tag_pk AND t.category_pk<>NEW.category_pk)
 BEGIN
 INSERT INTO known_image_tags(model_name,tag_en,tag_zh,image_count,updated_at)
 VALUES((SELECT value FROM storage_strings WHERE string_pk=NEW.model_pk),(SELECT value FROM storage_strings WHERE string_pk=NEW.tag_pk),(SELECT value FROM storage_strings WHERE string_pk=NEW.translation_pk),1,NEW.updated_at)
 ON CONFLICT(model_name,tag_en) DO UPDATE SET image_count=known_image_tags.image_count+1,tag_zh=COALESCE(excluded.tag_zh,known_image_tags.tag_zh),updated_at=excluded.updated_at;
 END;
CREATE TRIGGER storage_auto_tags_update_known AFTER UPDATE OF translation_pk,updated_at ON storage_auto_tags BEGIN
 UPDATE known_image_tags SET tag_zh=COALESCE((SELECT value FROM storage_strings WHERE string_pk=NEW.translation_pk),tag_zh),updated_at=NEW.updated_at
 WHERE model_name=(SELECT value FROM storage_strings WHERE string_pk=NEW.model_pk) AND tag_en=(SELECT value FROM storage_strings WHERE string_pk=NEW.tag_pk);
 END;
CREATE TRIGGER storage_auto_tags_delete_known AFTER DELETE ON storage_auto_tags
 WHEN NOT EXISTS(SELECT 1 FROM storage_auto_tags t WHERE t.image_pk=OLD.image_pk AND t.model_pk=OLD.model_pk AND t.tag_pk=OLD.tag_pk)
 BEGIN
 UPDATE known_image_tags SET image_count=MAX(0,image_count-1),updated_at=OLD.updated_at
 WHERE model_name=(SELECT value FROM storage_strings WHERE string_pk=OLD.model_pk) AND tag_en=(SELECT value FROM storage_strings WHERE string_pk=OLD.tag_pk);
 DELETE FROM known_image_tags WHERE model_name=(SELECT value FROM storage_strings WHERE string_pk=OLD.model_pk) AND tag_en=(SELECT value FROM storage_strings WHERE string_pk=OLD.tag_pk) AND image_count<=0;
 END;
