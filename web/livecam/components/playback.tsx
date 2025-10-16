import { useState, useEffect, useRef } from 'preact/hooks';
import { useAuth } from './auth';
import { FilmIcon, CalendarDaysIcon } from '@heroicons/react/24/outline';

interface PlaybackViewerProps {
  streamId: string;
}

interface RecordingFile {
  filename: string;
  url: string;
}

export function PlaybackViewer({ streamId }: PlaybackViewerProps) {
  const { token } = useAuth();
  const [availableDates, setAvailableDates] = useState<string[]>([]);
  const [selectedDate, setSelectedDate] = useState<string>('');
  const [recordings, setRecordings] = useState<RecordingFile[]>([]);
  const [selectedRecordingUrl, setSelectedRecordingUrl] = useState<string>('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const videoRef = useRef<HTMLVideoElement>(null);

  useEffect(() => {
    if (!streamId || !token) return;
    
    const fetchDates = async () => {
      setIsLoading(true);
      setError(null);
      try {
        const res = await fetch(`/api/recordings/${streamId}/dates`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (!res.ok) throw new Error('Failed to fetch available dates.');
        const dates: string[] = await res.json();
        setAvailableDates(dates.sort((a, b) => b.localeCompare(a))); 
        if (dates.length > 0) {
          setSelectedDate(dates[0]); 
        }
      } catch (e: unknown) {
          console.error('Error loading recordings:', e);
      } finally {
        setIsLoading(false);
      }
    };

    fetchDates();
  }, [streamId, token]);


  useEffect(() => {
    if (!selectedDate || !token) {
      setRecordings([]);
      return;
    };

    const fetchRecordings = async () => {
      setIsLoading(true);
      setError(null);
      setSelectedRecordingUrl(''); 
      try {
        const res = await fetch(`/api/recordings/${streamId}/${selectedDate}`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (!res.ok) throw new Error(`Failed to fetch recordings for ${selectedDate}.`);
        const files: RecordingFile[] = await res.json();
        setRecordings(files);
      } catch (e: unknown) {
        console.error('Error loading recordings:', e);
      } finally {
        setIsLoading(false);
      }
    };

    fetchRecordings();
  }, [selectedDate, token]);

  const handleSelectRecording = (file: RecordingFile) => {
    const urlWithToken = `${file.url}?token=${token}`;
    setSelectedRecordingUrl(urlWithToken);
  };

  return (
    <div className="bg-base-300 rounded-lg overflow-hidden shadow-lg">
      {/* video player */}
      <div className="relative aspect-video bg-black">
        {selectedRecordingUrl ? (
          <video ref={videoRef} src={selectedRecordingUrl} className="w-full h-full" controls autoPlay playsInline />
        ) : (
          <div className="w-full h-full flex flex-col items-center justify-center text-base-content">
            <FilmIcon className="w-16 h-16 opacity-30" />
            <p className="mt-4">Please select a recording to play.</p>
          </div>
        )}
      </div>

      {/* Control and list areas */}
      <div className="p-4 bg-base-200">
        <div className="flex items-center gap-4 mb-4">
          <label className="form-control w-full max-w-xs">
            <div className="label">
              <span className="label-text flex items-center gap-2"><CalendarDaysIcon className="w-4 h-4"/> Select Date</span>
            </div>
            <select
              className="select select-bordered"
              value={selectedDate}
              onChange={(e) => setSelectedDate(e.currentTarget.value)}
              disabled={isLoading || availableDates.length === 0}
            >
              <option disabled value="">
                {availableDates.length > 0 ? 'Pick a date' : 'No recordings found'}
              </option>
              {availableDates.map(date => <option key={date} value={date}>{date}</option>)}
            </select>
          </label>
        </div>
        
        {isLoading && <div className="text-center p-4"><span className="loading loading-spinner"></span></div>}
        {error && <div className="alert alert-error"><span>{error}</span></div>}

        {!isLoading && !error && recordings.length > 0 && (
          <div className="max-h-48 overflow-y-auto">
            <ul className="menu bg-base-100 rounded-box">
              {recordings.map(file => (
                <li key={file.filename}>
                  <a onClick={() => handleSelectRecording(file)}>
                    {file.filename}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        )}
         {!isLoading && !error && recordings.length === 0 && selectedDate && (
            <p className="text-center p-4 text-base-content opacity-70">No recordings for this date.</p>
         )}
      </div>
    </div>
  );
}
