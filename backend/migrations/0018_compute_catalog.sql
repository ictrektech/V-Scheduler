CREATE TABLE vrs_compute_profiles (
    name text PRIMARY KEY,
    builtin boolean NOT NULL DEFAULT false
);

INSERT INTO vrs_compute_profiles (name, builtin) VALUES
    ('jetson-thor', true), ('dgx-spark', true), ('jetson-orin', true),
    ('arm-cpu', true), ('amd-cpu', true), ('amd-cuda', true),
    ('arm-cuda', true), ('sophon-bm1688', true);

CREATE TABLE vrs_inference_images (
    image_ref text PRIMARY KEY,
    profiles text[] NOT NULL CHECK (cardinality(profiles) BETWEEN 1 AND 64),
    tested_profiles text[] NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- Entries observed on running Thor and Orin inference nodes. This seeds
-- metadata only. Image pulls remain part of the deployment workflow.
INSERT INTO vrs_inference_images (image_ref, profiles, tested_profiles) VALUES
    ('lispy.org/vllm/vllm-openai:v0.33.0-aarch64',
     ARRAY['jetson-thor'], ARRAY['jetson-thor']),
    ('swr.cn-southwest-2.myhuaweicloud.com/ictrek/vllm_server:aarch64_torch2.11.0_cuda13.0_v0.25.1_20260717',
     ARRAY['jetson-thor'], ARRAY['jetson-thor']),
    ('swr.cn-southwest-2.myhuaweicloud.com/ictrek/ollama_server:thor_20261012',
     ARRAY['jetson-thor'], ARRAY['jetson-thor']),
    ('swr.cn-southwest-2.myhuaweicloud.com/ictrek/ollama_server:thor_20261026',
     ARRAY['jetson-thor'], ARRAY['jetson-thor']),
    ('swr.cn-southwest-2.myhuaweicloud.com/ictrek/ollama_server:l4t_20261026',
     ARRAY['jetson-orin'], ARRAY['jetson-orin']);
