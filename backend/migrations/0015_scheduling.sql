-- Keep legacy requests at background priority; never rewrite their consent.
CREATE FUNCTION vrs_priority(options JSONB) RETURNS INTEGER LANGUAGE SQL IMMUTABLE AS $$
 SELECT CASE options->>'priority' WHEN 'high' THEN 2 WHEN 'interactive' THEN 1 ELSE 0 END
$$;
